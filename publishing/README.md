# Publishing to the OpenIDEA marketplace

Community plugins are listed through the registry
[l0ng-ai/openidea-plugins](https://github.com/l0ng-ai/openidea-plugins).

1. Once: `openidea-plugin keygen ~/fsantomero.key`. Keep the file secret and out
   of git (`*.key` is ignored); put the printed public key in
   `publishers/fsantomero.toml`.
2. Push this repository to `https://github.com/Santom24/openidea-maven-helper`
   (public, Apache-2.0).
3. `scripts/release.sh ~/fsantomero.key` builds, tests, signs and prints the
   package name and SHA-256.
4. Create the GitHub release `v<version>` and attach the `.oiplugin`.
5. Fork the registry, copy `publishers/fsantomero.toml` (first time only) and
   `plugins/fsantomero.maven-helper.toml` with the real SHA-256, open a pull
   request. CI checks the package; a maintainer reviews the source and the
   permissions (explained in the main README).

For a new version: bump `version` in `plugin.toml` and `Cargo.toml`, update
`change-notes`, run the script, release, and add a `[[versions]]` table.
