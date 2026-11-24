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
| `404.html` | not-found page (uses root-relative paths) |

Preview locally: `python3 -m http.server -d site 8000`, then open http://localhost:8000.

Deploy: publish this folder as the site root, for example GitHub Pages from a workflow that uploads `site/`, or Cloudflare Pages with `site` as the output directory and no build command.

Palette: crust `#3a2208`, honey `#f2b84b`, pastry `#c9852c`, syrup `#a96a1e`, pistachio `#8cc43f`, cream `#fff3d6`.
