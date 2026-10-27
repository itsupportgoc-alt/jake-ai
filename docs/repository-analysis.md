# Repository analysis

**Path:** project root (cwd)

project root (cwd) sits inside it as an untracked path. Committing inside it as a
separate repository would stage work into the home-directory repository.

**Decision:** initialise a dedicated repository *inside* the project so the
project history, lockfile and CI stay self-contained, and the surrounding
workspace stays clean.

## Key layout decisions

The layout below is a faithful adaptation of the master prompt's proposed
project layout, with the repository moved into `core/` and the crates renamed
to the `Jake-*` convention.

3. The master prompt's proposed project layout is adopted with adjustments noted in
   `docs/structure.md`.
