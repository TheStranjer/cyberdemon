# Images to assets

To be written. This guide will answer:

- What makes a usable source image for a texture, flat or sprite, and how to
  judge one before converting?
- How to pick the target size: 64 or 128 wide for wall textures, 64×64 for
  flats, what scale for a sprite given the thing's radius?
- How to handle colour against Doom's 256-colour palette: quantisation,
  dithering, and what to do with source colours outside the palette?
- How to make a texture tile cleanly, including the vertical repeat of 128?
- How to register a generated texture into the texture lumps and apply it to
  walls?
- How to verify the result in the engine (`render_view`) rather than only in
  the source image?
