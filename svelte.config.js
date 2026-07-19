import { vitePreprocess } from "@sveltejs/vite-plugin-svelte";

export default {
  // Enables <script lang="ts"> and other preprocessing.
  preprocess: vitePreprocess(),
};
