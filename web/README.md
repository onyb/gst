# gst landing page

The marketing site for the `gst` CLI: what it does, why it exists, and how
to build it. Vite + React + TypeScript, plain CSS (no framework).

## Develop

```sh
npm install
npm run dev
```

## Build

```sh
npm run build
```

Static output goes to `dist/`. There's no deployment workflow wired up yet
— `dist/` is ready to serve as-is from GitHub Pages or any static host.

## Structure

- `src/components/` — one section of the page per component, each with its
  own CSS file
- `src/content/commands.ts` — the CLI's subcommand list; mirrors
  `crates/gst-cli/src/main.rs`'s `Command` enum, keep the two in sync
- `src/styles/tokens.css` — the color/type/spacing design tokens
- `src/hooks/` — the typing-terminal and scroll-reveal effects, both of
  which respect `prefers-reduced-motion`

Fonts (IBM Plex Sans/Mono) are self-hosted via `@fontsource`, latin subset
only — no external font requests.
