# poly documentation site

The [poly](https://github.com/polylint/polylint) documentation, built with
[Astro](https://astro.build) and [Starlight](https://starlight.astro.build) and deployed to GitHub
Pages at <https://polylint.github.io/polylint/>.

```sh
pnpm install
pnpm run dev      # local dev server
pnpm run build    # static build into dist/
pnpm run preview  # serve the built site
```

Because the site is hosted on a GitHub Pages *project* page, `astro.config.mjs` sets
`base: "/polylint"`. Every internal link in the content must therefore be written with that prefix
(`/polylint/reference/cli/`), and `dist/` is served at `https://polylint.github.io/polylint/` — the build
does not emit a `dist/polylint/` subdirectory.

`.github/workflows/docs.yaml` builds and deploys on every push to `main` that touches `website/**`.
