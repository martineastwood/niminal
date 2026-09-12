# niminal

The umbrella landing page for the Niminal stack, served at
[niminal.dev](https://niminal.dev).

Each project documents itself on its own subdomain:

| Project | Docs |
| --- | --- |
| nimlet | https://nimlet.niminal.dev |
| nimgent | https://nimgent.niminal.dev |
| nimterm | https://nimterm.niminal.dev |
| nimwire | https://nimwire.niminal.dev |

## Development

```sh
npm install
npm run dev
```

Open the local URL printed by Astro. Production builds use `npm run build`.

The page lives in `src/pages/index.astro`. Docs content belongs in the project
repositories, not here.
