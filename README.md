# niminal

The landing page for the Niminal stack, served at
[niminal.dev](https://niminal.dev).

It is a static site with no build step and no dependencies. Page markup lives
in `index.html`. Images, CSS, and the web manifest live in `assets/`.

Open `index.html` in a browser to preview it, or serve the directory:

```sh
python3 -m http.server 4321
```

Pushes to `main` deploy the site to GitHub Pages via
`.github/workflows/pages.yml`. Enable GitHub Pages for this repository with
**GitHub Actions** as the source.

The page links to each project's own documentation site:

| Project | Docs | Source |
| --- | --- | --- |
| nimlet | https://nimlet.niminal.dev | https://github.com/martineastwood/nimlet |
| nimgent | https://nimgent.niminal.dev | https://github.com/martineastwood/nimgent |
| nimwire | https://nimwire.niminal.dev | https://github.com/martineastwood/nimwire |
| nimterm | https://nimterm.niminal.dev | https://github.com/martineastwood/nimterm |

Project documentation lives in the project repositories, not here.

## License

MIT. See [LICENSE](LICENSE).
