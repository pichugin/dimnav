//! Directory-watching runtime — the Tauri-side host that keeps the panels in
//! step with the filesystem (SPEC §5.6).
//!
//! `fm-core` decides what a change *means* ([`fm_core::fs::watch`]) and owns the
//! reconciliation ([`fm_core::nav::set_listing_preserving`]); this module does
//! the OS side of it, because watchers, threads and events are adapter concerns
//! (SPEC §3). It implements [`fm_core::plugin::FsObserver`], so the change source
//! is replaceable without the panels noticing.
//!
//! ## One thread, one owner, no second lock
//!
//! Everything — filesystem events, re-arm requests from `navigate`, the focus
//! poke, and the periodic tick — arrives on a single channel and is handled by
//! one thread that exclusively owns the watchers, the directory handles and the
//! digests. Nothing here is shared, so there is no watcher mutex that could
//! deadlock against [`SharedState`]: the only lock taken is the app state's, and
//! never across a directory read.
//!
//! ## Events never map 1:1 to re-listings
//!
//! A `git checkout` or an archive extraction produces thousands of events. An
//! event only marks a panel **dirty**; the tick decides when to actually re-read,
//! after `debounce_ms` of quiet or `max_delay_ms` at the latest. The cap is what
//! keeps a sustained write (which never goes quiet) from showing nothing until it
//! finishes. A re-read that produces a byte-identical listing is dropped before it
//! becomes an event, so an event storm that changes nothing visible costs one
//! directory read and no IPC traffic at all.
//!
//! ## Why there is a poll as well as a watcher
//!
//! Renaming a *grandparent* of the open directory changes nothing inside either
//! watched directory, so no event fires — but the panel's path is now wrong. The
//! identity probe (two syscalls) catches that, which is why it runs on a timer as
//! well as on every event.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use fm_core::fs::watch::{
    decide, nearest_readable_ancestor, recover, DirFate, DirHandle, DirRecovery, PanelNoticeSpec,
};
use fm_core::plugin::FsObserver;
use fm_core::types::{DirListing, PanelChanged, PanelId, PanelNoticeKind, SortMode, WatchPrefs};
use notify::{Config as NotifyConfig, PollWatcher, RecommendedWatcher, RecursiveMode, Watcher};
use tauri::{AppHandle, Manager};
use tauri_specta::Event;

use crate::commands::{home_dir, SharedState};
use crate::events::PanelChangedEvent;

/// How often the service thread wakes when nothing is happening. Fine enough to
/// honour a 200 ms debounce without busy-waiting.
const TICK: Duration = Duration::from_millis(50);

/// Messages into the single service thread.
enum WatchCmd {
    /// A panel now shows a different directory.
    Observe { panel: PanelId, path: PathBuf },
    /// A panel is no longer being shown.
    Release(PanelId),
    /// The filesystem reported a change at this path.
    Changed(PathBuf),
    /// Check everything right now (window focus).
    Poke,
}

/// Tauri-managed handle to the watcher thread.
///
/// Sending is the only thing callers can do, which is what keeps this free of
/// lock-ordering hazards against [`SharedState`].
pub struct WatchRuntime {
    tx: Mutex<Option<Sender<WatchCmd>>>,
}

impl Default for WatchRuntime {
    fn default() -> Self {
        Self {
            tx: Mutex::new(None),
        }
    }
}

impl WatchRuntime {
    /// Start the service thread. Called once from `setup`.
    pub fn start(&self, app: AppHandle) {
        let (tx, rx) = channel();
        *self.tx.lock().expect("watch runtime poisoned") = Some(tx.clone());
        std::thread::spawn(move || Service::new(app, tx).run(rx));
    }

    fn send(&self, cmd: WatchCmd) {
        if let Ok(guard) = self.tx.lock() {
            if let Some(tx) = guard.as_ref() {
                // A closed channel means the app is shutting down; watching is a
                // convenience, so there is nothing useful to report.
                let _ = tx.send(cmd);
            }
        }
    }
}

impl FsObserver for WatchRuntime {
    fn observe(&self, panel: PanelId, path: &Path) {
        self.send(WatchCmd::Observe {
            panel,
            path: path.to_path_buf(),
        });
    }

    fn release(&self, panel: PanelId) {
        self.send(WatchCmd::Release(panel));
    }

    fn poke(&self) {
        self.send(WatchCmd::Poke);
    }
}

/// What the service thread knows about one panel.
struct PanelWatch {
    path: PathBuf,
    /// Live reference to the directory, for telling "renamed" from "deleted".
    /// `None` when it could not be opened (a platform without support, or a
    /// directory that vanished before we got to it).
    handle: Option<DirHandle>,
    /// Digest of the listing last pushed to the frontend, so an event storm that
    /// changes nothing visible produces no IPC traffic.
    digest: u64,
    /// When the first un-serviced event for this panel arrived (drives the
    /// max-delay cap), and when the most recent one did (drives the debounce).
    dirty_since: Option<Instant>,
    last_event: Option<Instant>,
    /// When the identity of this directory was last probed.
    last_probe: Instant,
}

impl PanelWatch {
    fn new(path: PathBuf) -> Self {
        let handle = DirHandle::open(&path).ok();
        Self {
            path,
            handle,
            digest: 0,
            dirty_since: None,
            last_event: None,
            // Probe immediately on the first tick so a directory that changed
            // while the app was not watching is caught at once.
            last_probe: Instant::now() - Duration::from_secs(3600),
        }
    }

    fn mark_dirty(&mut self, now: Instant) {
        self.dirty_since.get_or_insert(now);
        self.last_event = Some(now);
    }

    fn clear_dirty(&mut self) {
        self.dirty_since = None;
        self.last_event = None;
    }

    /// Is this panel due for servicing?
    fn due(&self, now: Instant, prefs: &WatchPrefs, forced: bool) -> bool {
        if forced {
            return true;
        }
        if now.duration_since(self.last_probe) >= Duration::from_millis(prefs.identity_poll_ms) {
            return true;
        }
        match (self.dirty_since, self.last_event) {
            (Some(since), Some(last)) => {
                let quiet = now.duration_since(last) >= Duration::from_millis(prefs.debounce_ms);
                let capped =
                    now.duration_since(since) >= Duration::from_millis(prefs.max_delay_ms);
                quiet || capped
            }
            _ => false,
        }
    }
}

/// The service thread's exclusively-owned world.
struct Service {
    app: AppHandle,
    tx: Sender<WatchCmd>,
    panels: HashMap<PanelId, PanelWatch>,
    /// FSEvents (or the platform equivalent) for ordinary local volumes.
    local: Option<RecommendedWatcher>,
    /// Fallback for network mounts, which FSEvents does not report on at all —
    /// it creates the stream happily and then never delivers anything.
    remote: Option<PollWatcher>,
    /// Paths currently registered, and with which watcher.
    watched: HashMap<PathBuf, WatcherKind>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WatcherKind {
    Local,
    Remote,
}

impl Service {
    fn new(app: AppHandle, tx: Sender<WatchCmd>) -> Self {
        Self {
            app,
            tx,
            panels: HashMap::new(),
            local: None,
            remote: None,
            watched: HashMap::new(),
        }
    }

    fn run(mut self, rx: Receiver<WatchCmd>) {
        loop {
            let mut forced = false;
            match rx.recv_timeout(TICK) {
                Ok(cmd) => {
                    forced = self.handle(cmd);
                    // Drain anything else already queued before doing work, so a
                    // burst of events costs one pass rather than one pass each.
                    while let Ok(next) = rx.try_recv() {
                        forced |= self.handle(next);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                // The sender is gone: the app is shutting down.
                Err(RecvTimeoutError::Disconnected) => return,
            }

            let prefs = self.prefs();
            if !prefs.enabled {
                continue;
            }
            let now = Instant::now();
            let due: Vec<PanelId> = self
                .panels
                .iter()
                .filter(|(_, w)| w.due(now, &prefs, forced))
                .map(|(id, _)| *id)
                .collect();
            for panel in due {
                self.service(panel, &prefs);
            }
        }
    }

    /// Apply one message. Returns whether it demands an immediate pass.
    fn handle(&mut self, cmd: WatchCmd) -> bool {
        match cmd {
            WatchCmd::Observe { panel, path } => {
                let unchanged = self
                    .panels
                    .get(&panel)
                    .is_some_and(|w| w.path == path);
                if !unchanged {
                    self.panels.insert(panel, PanelWatch::new(path));
                    self.rearm();
                }
                false
            }
            WatchCmd::Release(panel) => {
                self.panels.remove(&panel);
                self.rearm();
                false
            }
            WatchCmd::Changed(path) => {
                let now = Instant::now();
                for w in self.panels.values_mut() {
                    // A panel cares about a change at its own path, inside it, or
                    // at an ancestor (which is how it learns it was renamed).
                    if path.starts_with(&w.path) || w.path.starts_with(&path) {
                        w.mark_dirty(now);
                    }
                }
                false
            }
            WatchCmd::Poke => true,
        }
    }

    /// Bring the registered watches in line with what the panels are showing.
    ///
    /// The panel's directory *and its parent* are both watched: the parent is what
    /// reports the directory itself being renamed or removed. Deduplicated, so two
    /// panels in the same place cost one watch.
    fn rearm(&mut self) {
        let mut wanted: HashSet<PathBuf> = HashSet::new();
        for w in self.panels.values() {
            wanted.insert(w.path.clone());
            if let Some(parent) = w.path.parent() {
                wanted.insert(parent.to_path_buf());
            }
        }

        let stale: Vec<PathBuf> = self
            .watched
            .keys()
            .filter(|p| !wanted.contains(*p))
            .cloned()
            .collect();
        for path in stale {
            if let Some(kind) = self.watched.remove(&path) {
                match kind {
                    WatcherKind::Local => {
                        if let Some(w) = self.local.as_mut() {
                            let _ = w.unwatch(&path);
                        }
                    }
                    WatcherKind::Remote => {
                        if let Some(w) = self.remote.as_mut() {
                            let _ = w.unwatch(&path);
                        }
                    }
                }
            }
        }

        let prefs = self.prefs();
        for path in wanted {
            if self.watched.contains_key(&path) {
                continue;
            }
            let kind = if is_local_volume(&path) {
                WatcherKind::Local
            } else {
                WatcherKind::Remote
            };
            if kind == WatcherKind::Remote && prefs.poll_non_local_ms == 0 {
                continue; // polling disabled: Ctrl+R and the focus refresh cover it
            }
            if self.ensure_watcher(kind, &prefs).is_none() {
                continue;
            }
            let ok = match kind {
                WatcherKind::Local => self
                    .local
                    .as_mut()
                    .map(|w| w.watch(&path, RecursiveMode::NonRecursive).is_ok()),
                WatcherKind::Remote => self
                    .remote
                    .as_mut()
                    .map(|w| w.watch(&path, RecursiveMode::NonRecursive).is_ok()),
            };
            // A directory we cannot watch is not an error worth surfacing: the
            // identity poll still covers it, as do Ctrl+R and the focus refresh.
            if ok == Some(true) {
                self.watched.insert(path, kind);
            }
        }
    }

    /// Create the watcher of the given kind if it does not exist yet.
    fn ensure_watcher(&mut self, kind: WatcherKind, prefs: &WatchPrefs) -> Option<()> {
        let tx = self.tx.clone();
        let handler = move |res: Result<notify::Event, notify::Error>| {
            if let Ok(ev) = res {
                for p in ev.paths {
                    let _ = tx.send(WatchCmd::Changed(p));
                }
            }
        };
        match kind {
            WatcherKind::Local => {
                if self.local.is_none() {
                    self.local = RecommendedWatcher::new(handler, NotifyConfig::default()).ok();
                }
                self.local.as_ref().map(|_| ())
            }
            WatcherKind::Remote => {
                if self.remote.is_none() {
                    let cfg = NotifyConfig::default()
                        .with_poll_interval(Duration::from_millis(prefs.poll_non_local_ms));
                    self.remote = PollWatcher::new(handler, cfg).ok();
                }
                self.remote.as_ref().map(|_| ())
            }
        }
    }

    fn prefs(&self) -> WatchPrefs {
        self.app
            .state::<SharedState>()
            .lock()
            .map(|s| s.config.watch.clone())
            .unwrap_or_default()
    }

    /// Probe one panel and apply whatever the core decides.
    fn service(&mut self, panel: PanelId, prefs: &WatchPrefs) {
        let now = Instant::now();
        if let Some(w) = self.panels.get_mut(&panel) {
            w.last_probe = now;
            w.clear_dirty();
        }

        // Read what we need, then drop the lock before touching the filesystem.
        let state = self.app.state::<SharedState>();
        let Some((path, show_hidden, sort)) = ({
            let Ok(s) = state.lock() else { return };
            let p = s.panel(panel);
            Some((p.path.clone(), p.show_hidden, p.sort_mode))
        }) else {
            return;
        };

        let home = home_dir();
        let fate = match self.panels.get(&panel).and_then(|w| w.handle.as_ref()) {
            Some(h) => {
                let obs = h.observe(&path);
                decide(&obs, &path, Path::new(&home), prefs.follow_moves)
            }
            // No handle: fall back to a plain re-read, which is still better than
            // showing a stale listing.
            None => DirFate::Alive,
        };

        let recovery = recover(fate, &path, prefs, &home, nearest_readable_ancestor);
        self.apply(panel, recovery, show_hidden, sort);
    }

    fn apply(&mut self, panel: PanelId, recovery: DirRecovery, show_hidden: bool, sort: SortMode) {
        // Where to read, and how the listing should land.
        enum Land {
            Preserve,
            Fresh,
            At(Option<String>),
        }
        let (target, land, notice) = match recovery {
            DirRecovery::Reload => (None, Land::Preserve, None),
            DirRecovery::ReloadFresh(n) => (None, Land::Fresh, Some(n)),
            DirRecovery::Follow { path, notice } => (Some(path), Land::Preserve, Some(notice)),
            DirRecovery::Fallback {
                path,
                cursor_hint,
                notice,
            } => (Some(path), Land::At(cursor_hint), Some(notice)),
            DirRecovery::Hold(n) => {
                self.hold(panel, n);
                return;
            }
        };

        let moved = target.is_some();
        let path = match &target {
            Some(p) => p.clone(),
            None => match self.panels.get(&panel) {
                Some(w) => w.path.to_string_lossy().into_owned(),
                None => return,
            },
        };

        let listing = fm_core::fs::list_dir(&path, show_hidden, sort);
        let digest = digest_of(&listing);

        // Nothing visibly changed and the panel is where it should be: drop it
        // here rather than serialising a whole listing across the IPC boundary.
        // The one thing that still needs a pass is a stale "permission denied"
        // notice, which a successful re-read has just disproved.
        if !moved && matches!(land, Land::Preserve) {
            if let Some(w) = self.panels.get(&panel) {
                let denied_pending = self
                    .app
                    .state::<SharedState>()
                    .lock()
                    .map(|s| {
                        matches!(
                            s.panel(panel).notice.as_ref().map(|n| n.kind),
                            Some(PanelNoticeKind::Denied)
                        )
                    })
                    .unwrap_or(false);
                if w.digest == digest && !denied_pending {
                    return;
                }
            }
        }

        let state = self.app.state::<SharedState>();
        let changed = {
            let Ok(mut s) = state.lock() else { return };
            let p = s.panel_mut(panel);
            match land {
                Land::Preserve => fm_core::nav::set_listing_preserving(p, listing),
                Land::Fresh => fm_core::nav::set_listing(p, listing),
                Land::At(hint) => {
                    fm_core::nav::set_listing(p, listing);
                    if let Some(name) = hint {
                        fm_core::nav::position_on_nearest_sorted(p, &name);
                    }
                }
            }
            match notice {
                Some(spec) => p.notice = Some(spec.into_notice()),
                // An ordinary refresh must not wipe the notice explaining how the
                // panel got here — the events that follow a move would otherwise
                // clear it within a couple of hundred milliseconds, long before
                // anyone read it. It survives until the user navigates.
                //
                // "Permission denied" is the exception: it describes a *current*
                // condition rather than something that already happened, and a
                // successful re-read has just disproved it.
                None => {
                    if matches!(
                        p.notice.as_ref().map(|n| n.kind),
                        Some(PanelNoticeKind::Denied)
                    ) {
                        p.notice = None;
                    }
                }
            }

            // Cached folder sizes under a directory that changed on disk are no
            // longer trustworthy.
            let base = PathBuf::from(&s.panel(panel).path);
            s.revalidate_sizes_under(&base);

            if moved {
                // The panel's directory changed, so the remembered start_dir
                // would otherwise point at a path that no longer exists (§7).
                s.sync_prefs_from_panels();
                let config = s.config.clone();
                tauri::async_runtime::spawn_blocking(move || fm_core::config::save(&config));
            }
            s.panel_snapshot(panel)
        };

        if let Some(w) = self.panels.get_mut(&panel) {
            w.digest = digest;
            if moved {
                w.path = PathBuf::from(&path);
                w.handle = DirHandle::open(&w.path).ok();
            }
        }
        if moved {
            self.rearm();
        }

        let _ = PanelChangedEvent(PanelChanged {
            panel,
            state: changed,
        })
        .emit(&self.app);
    }

    /// Attach a notice without moving the panel (the unreadable case). Emitted
    /// only when it is new, so a directory that stays unreadable does not produce
    /// an event on every poll.
    fn hold(&mut self, panel: PanelId, spec: PanelNoticeSpec) {
        let notice = spec.into_notice();
        let state = self.app.state::<SharedState>();
        let Ok(mut s) = state.lock() else { return };
        let p = s.panel_mut(panel);
        if p.notice.as_ref().is_some_and(|n| {
            n.kind == notice.kind && n.path == notice.path
        }) {
            return;
        }
        p.notice = Some(notice);
        let snapshot = s.panel_snapshot(panel);
        drop(s);

        let _ = PanelChangedEvent(PanelChanged {
            panel,
            state: snapshot,
        })
        .emit(&self.app);
    }
}

/// Cheap fingerprint of everything a panel actually renders, so a re-read that
/// changed nothing visible can be dropped before it becomes IPC traffic.
fn digest_of(listing: &DirListing) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    listing.path.hash(&mut h);
    // A directory flipping to unreadable keeps an empty entry list, so without
    // this the change hashes identically and the panel never hears about it.
    listing.access.as_ref().map(|a| a.kind).hash(&mut h);
    listing.entries.len().hash(&mut h);
    for e in &listing.entries {
        e.name.hash(&mut h);
        e.size.hash(&mut h);
        e.modified.hash(&mut h);
        e.kind.hash(&mut h);
        e.permissions.hash(&mut h);
        e.marker.hash(&mut h);
    }
    h.finish()
}

/// Is this path on a local volume?
///
/// FSEvents only reports on local filesystems; on an SMB or NFS mount it creates
/// the stream without complaint and then never delivers an event, so those need
/// the poll fallback instead.
#[cfg(unix)]
fn is_local_volume(path: &Path) -> bool {
    #[cfg(target_os = "macos")]
    {
        let Ok(c) = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()) else {
            return true;
        };
        // SAFETY: `c` is a valid NUL-terminated path and `buf` is a correctly
        // sized, fully-written-by-statfs structure on success.
        let mut buf = std::mem::MaybeUninit::<libc::statfs>::uninit();
        let rc = unsafe { libc::statfs(c.as_ptr(), buf.as_mut_ptr()) };
        if rc != 0 {
            return true; // unknown: treat as local rather than start polling
        }
        let st = unsafe { buf.assume_init() };
        st.f_flags & (libc::MNT_LOCAL as u32) != 0
    }
    #[cfg(not(target_os = "macos"))]
    {
        // Phase 4: Linux compares statfs.f_type against the network-fs magics.
        let _ = path;
        true
    }
}

#[cfg(not(unix))]
fn is_local_volume(_path: &Path) -> bool {
    // Phase 4: Windows uses GetDriveType.
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use fm_core::types::{Entry, EntryCategory, EntryKind, EntryMarker};

    fn entry(name: &str, size: u64) -> Entry {
        Entry {
            name: name.to_string(),
            kind: EntryKind::File,
            size,
            modified: 100,
            created: 0,
            permissions: 0o644,
            uid: 0,
            gid: 0,
            owner: None,
            group: None,
            nlink: 1,
            symlink_target: None,
            is_executable: false,
            category: EntryCategory::Plain,
            marker: EntryMarker::Ok,
            computed_size: None,
        }
    }

    fn listing(entries: Vec<Entry>) -> DirListing {
        DirListing {
            path: "/dir".to_string(),
            entries,
            access: None,
        }
    }

    #[test]
    fn digest_ignores_nothing_that_shows_on_screen() {
        let base = listing(vec![entry("a", 1), entry("b", 2)]);
        assert_eq!(digest_of(&base), digest_of(&listing(vec![entry("a", 1), entry("b", 2)])));

        // Each of these is visible in a panel and must invalidate the digest.
        assert_ne!(digest_of(&base), digest_of(&listing(vec![entry("a", 1)])));
        assert_ne!(
            digest_of(&base),
            digest_of(&listing(vec![entry("a", 9), entry("b", 2)]))
        );
        assert_ne!(
            digest_of(&base),
            digest_of(&listing(vec![entry("a", 1), entry("z", 2)]))
        );

        let mut touched = listing(vec![entry("a", 1), entry("b", 2)]);
        touched.entries[0].modified = 999;
        assert_ne!(digest_of(&base), digest_of(&touched));

        // A directory turning unreadable keeps an empty entry list, so without
        // the access state in the digest the change would hash identically to an
        // empty directory and never reach the panel (§5.6).
        let empty = listing(vec![]);
        let mut denied = listing(vec![]);
        denied.access = Some(fm_core::types::DirAccessError {
            kind: fm_core::types::DirAccessKind::Denied,
            message: "nope".to_string(),
            remedies: Vec::new(),
        });
        assert_ne!(digest_of(&empty), digest_of(&denied));
    }

    #[test]
    fn debounce_waits_for_quiet_then_gives_up_waiting() {
        let prefs = WatchPrefs {
            debounce_ms: 200,
            max_delay_ms: 1000,
            identity_poll_ms: 60_000, // out of the way for this test
            ..WatchPrefs::default()
        };
        let mut w = PanelWatch::new(PathBuf::from("/nonexistent-for-test"));
        w.last_probe = Instant::now();

        let t0 = Instant::now();
        w.mark_dirty(t0);

        // Still inside the quiet period.
        assert!(!w.due(t0 + Duration::from_millis(100), &prefs, false));
        // Quiet long enough.
        assert!(w.due(t0 + Duration::from_millis(250), &prefs, false));

        // A continuous stream of events never goes quiet, so the cap must fire.
        let mut w2 = PanelWatch::new(PathBuf::from("/nonexistent-for-test"));
        w2.last_probe = Instant::now();
        w2.mark_dirty(t0);
        for ms in (0..1200).step_by(50) {
            w2.mark_dirty(t0 + Duration::from_millis(ms));
        }
        assert!(
            w2.due(t0 + Duration::from_millis(1200), &prefs, false),
            "max_delay_ms must break a sustained write stream"
        );
    }

    #[test]
    fn a_clean_panel_is_not_due_until_the_identity_poll() {
        let prefs = WatchPrefs {
            identity_poll_ms: 2000,
            ..WatchPrefs::default()
        };
        let mut w = PanelWatch::new(PathBuf::from("/nonexistent-for-test"));
        let t0 = Instant::now();
        w.last_probe = t0;

        assert!(!w.due(t0 + Duration::from_millis(500), &prefs, false));
        // The poll is what catches a renamed ancestor, which fires no event.
        assert!(w.due(t0 + Duration::from_millis(2100), &prefs, false));
        // A focus poke overrides both timers.
        assert!(w.due(t0, &prefs, true));
    }
}
