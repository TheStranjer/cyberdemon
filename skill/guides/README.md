# Guides

Guides hold the judgment side of Cyberdemon's division of labour. The server
performs formulaic operations on explicit inputs and never guesses; you — the
model doing the editing — interpret requests and images and choose the values
those tools ask for. When a parameter calls for a dimension, a light level or
a layout, the answer comes from these pages, not from the server.

| Guide | What it covers |
|-------|----------------|
| [design-rules.md](design-rules.md) | Doom dimensions and gameplay facts to design against |
| [verification.md](verification.md) | The change → validate → render → compare → fix loop |
| [sketch-to-map.md](sketch-to-map.md) | Turning a hand-drawn map into a playable layout |
| [images-to-assets.md](images-to-assets.md) | Turning drawn images into textures, flats and sprites |
| [editing-existing-maps.md](editing-existing-maps.md) | Changing maps that already exist in a WAD |
| [recipes.md](recipes.md) | Worked end-to-end examples of common requests |

Every row above links a file in this directory; a test in
`tests/mcp/docs.rs` checks the index and the directory agree.
