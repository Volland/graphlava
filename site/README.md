# graphlava.org

The static website for graphlava: plain HTML and one stylesheet. No build step, no JavaScript, no external fonts or trackers.

| Path | Content |
| --- | --- |
| `index.html` | home page |
| `articles/` | articles: statement graphs, graphlava and tiramemsu, graphlava and Neptune OneGraph |
| `book/` | mini book: hypergraphs and metagraphs in statement graphs (11 chapters) |
| `assets/logo.svg` | logo mark (baklava diamond, pastry layers, pistachio hyperedge) |
| `assets/logo-wordmark.svg` | mark with the word "graphlava" (system font) |
| `assets/favicon.svg` | simplified mark for 16–32 px |
| `CNAME` | `graphlava.org`, for GitHub Pages |
| `impressum.html`, `datenschutz.html`, `agb.html` | German legal pages (Impressum, Datenschutzerklärung, AGB), linked from every footer |
| `author.html` | mini author page, linked from the nav; links to pavlyshyn.me |
| `404.html` | not-found page (uses root-relative paths) |

Preview locally: `python3 -m http.server -d site 8000`, then open http://localhost:8000.

Deploy: every push to `main` that touches `site/` runs [.github/workflows/pages.yml](../.github/workflows/pages.yml). It checks all internal links, uploads this folder and publishes it with GitHub Pages (source: GitHub Actions, custom domain `graphlava.org`). It can also be run by hand from the Actions tab.

DNS for `graphlava.org` (Cloudflare, DNS only, proxy off):

| Type | Name | Content |
| --- | --- | --- |
| A | `@` | `185.199.108.153`, `185.199.109.153`, `185.199.110.153`, `185.199.111.153` (four records) |
| AAAA | `@` | `2606:50c0:8000::153`, `2606:50c0:8001::153`, `2606:50c0:8002::153`, `2606:50c0:8003::153` (four records) |
| CNAME | `www` | `volland.github.io` |

Once DNS resolves and GitHub has issued the certificate, turn on "Enforce HTTPS" in Settings → Pages.

Palette: crust `#3a2208`, honey `#f2b84b`, pastry `#c9852c`, syrup `#a96a1e`, pistachio `#8cc43f`, cream `#fff3d6`.
