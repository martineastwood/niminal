# niminal

The landing page for the Niminal stack, served at
[niminal.dev](https://niminal.dev).

It is a static site: `index.html`, `styles.css`, `site.webmanifest`, and the
favicon files (`favicon.svg`, `favicon.ico`, and the generated PNGs). There is no
build step and no dependencies. Open `index.html` in a browser to preview it, or
serve the directory:

```sh
python3 -m http.server 4321
```

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
