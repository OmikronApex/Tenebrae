# `bistro.tbsc`: attribution and changes

`bistro.tbsc` is derived from **Amazon Lumberyard Bistro** (v5.2), Open Research Content Archive
(ORCA), by Amazon Lumberyard, 2017: <http://developer.nvidia.com/orca/amazon-lumberyard-bistro>.

It is licensed under the **Creative Commons Attribution 4.0 International** licence (CC BY 4.0):
<https://creativecommons.org/licenses/by/4.0/>. The adapted material in this file is offered under
the same licence. No endorsement by Amazon or NVIDIA is implied.

Suggested citation (from the original README):

```
@misc{ORCAAmazonBistro,
  title = {Amazon Lumberyard Bistro, Open Research Content Archive (ORCA)},
  author = {Amazon Lumberyard},
  year = {2017},
  month = {July},
  url = {http://developer.nvidia.com/orca/amazon-lumberyard-bistro}
}
```

## Changes from the original

`BistroExterior.fbx` and `BistroInterior.fbx` were imported into Blender 5.2.2 (they share one
coordinate space; Z up, metres) and reduced to the geometry a visibility baker culls against:

- Every texture, material, normal, UV, light, camera and animation was removed; only triangle
  positions remain, in world space.
- Each source mesh object became one benchmark object: its box over all its triangles is the
  target, its occluder triangles are kept.
- Triangles of see-through materials do not occlude and were removed (their objects stay as
  targets): foliage cards (any material named `Foliage` except trunks, and `Plants` except bases),
  glass (`Glass`), string lights (`StringLight`) and water (`Water`).
- Vertices closer than 1 mm were merged, coplanar faces within 1° were dissolved, and the meshes
  were re-triangulated.
- Every object with more than 200 triangles left was decimated to a quarter of its triangles
  (Blender's Decimate modifier, collapse, ratio 0.25). The result has 653,286 triangles (2,365,425
  before decimation). Visibility is preserved closely: with 200 cameras drawn by the benchmark's
  rule from each version, the ray-cast truth finds 195.2 truly visible objects per view, against
  192.4 at full detail (1.5 %).
- Positions are quantised to 16 bits per axis over each object's own box (the TBSC format,
  `crates/tenebrae-bench/src/tbsc.rs`).
