# `sponza.tbsc`: attribution and changes

`sponza.tbsc` is derived from the **Crytek Sponza** model ("The Atrium Sponza Palace, Dubrovnik"),
© 2010 Frank Meinl, Crytek, after an original Sponza model by Marko Dabrovic (2002), in the version
Morgan McGuire corrected in 2011, as distributed by the McGuire Computer Graphics Archive:
<https://casual-effects.com/data/> (`sponza.zip`).

It is licensed under the **Creative Commons Attribution 3.0 Unported** licence (CC BY 3.0):
<https://creativecommons.org/licenses/by/3.0/>. The adapted material in this file is offered under
the same licence. No endorsement by Crytek, Frank Meinl or Morgan McGuire is implied.

The licence is stated by the archive's listing (also quoted in the third-party notices of Unity's
Classic Sponza remaster, <https://github.com/Unity-Technologies/Classic-Sponza>); the archive's
`copyright.txt` names the authors and records McGuire's corrections. Unity's remaster itself is
not used: its repository grants no licence for its own additions, and its geometry derives from
this model.

## Changes from the original

`sponza.obj` was imported into Blender 5.2.2 (OBJ importer, Y up converted to Z up) and reduced
to the geometry a visibility baker culls against:

- Positions were scaled from centimetres to metres (× 0.01).
- Every texture, material, normal and UV was removed; only triangle positions remain, in world
  space.
- Each OBJ group became one benchmark object (381): its box over all its triangles is the target,
  its occluder triangles are kept.
- Triangles of the three alpha-masked materials do not occlude and were removed (their objects
  stay as targets): `leaf` (thorn plants), `Material__57` (vase plants) and `chain`.
- Vertices closer than 1 mm were merged, coplanar faces within 1° were dissolved, and the meshes
  were re-triangulated: 194,187 occluder triangles. Nothing was decimated.
- Positions are quantised to 16 bits per axis over each object's own box (the TBSC format,
  `crates/tenebrae-bench/src/tbsc.rs`).
