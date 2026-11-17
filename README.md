# Maven Helper for OpenIDEA

Run and debug Maven goals from the right-click menu, keep your own goals, and
fix dependency conflicts from a tab on `pom.xml`, for
[OpenIDEA](https://openidea.sh).

Inspired by [Maven Helper](https://github.com/krasa/MavenHelper) for IntelliJ
IDEA by Vojtech Krasa. This is an independent implementation on the OpenIDEA
plugin API: it contains no code, icons or other material from that project.

## Features

### Run Maven / Debug Maven

Right-click a file or folder in the Project view, inside an editor, or on an
editor tab:

- **Run Maven ▸** clean, validate, compile, test, package, verify, install,
  clean install, deploy, site, dependency:tree; *Saved Goals…*, *Custom Goal…*,
  *Goal on Root Module…*, *Re-run Last Goal*, *Reimport*, *New Goal…*,
  *Remove Saved Goals…*.
- **Debug Maven ▸** the same goals, *Saved Goals…*, *Custom Goal…*, *New Goal…*.
- Also in **Tools**, in Find Action (categories *Run Maven*, *Debug Maven*) and
  on `Ctrl+Alt+R` (Saved Goals…).

The module is the nearest `pom.xml` above what you clicked (or the focused
file); *Goal on Root Module…* takes the topmost one. A folder holding several
modules and no POM of its own asks which module.

Goals run as `mvn <goals> -f <pom> [extra arguments] -Dstyle.color=always` in
the **Run tool window** (Stop, Rerun, coloured output), through the plugin's
run configurations: one per menu goal (`Maven: clean install`), one per saved
goal, and a generic `Maven`. If the IDE hasn't asked for them yet, a goal runs
in a log tab that stays open with its output and exit code.

**New Goal…** asks for goals and options (`clean install -DskipTests`), saves
them and runs them. Saved goals lead the *Saved Goals…* list and appear as
run configurations; *Remove Saved Goals…* deletes one or more.

**Debug Maven** adds `-Dmaven.surefire.debug`, `-Dmaven.failsafe.debug` and
`-Dspring-boot.run.jvmArguments` with
`-agentlib:jdwp=transport=dt_socket,server=y,suspend=y,address=5005`: the test
JVMs and `spring-boot:run` wait for a debugger. Attach a *Remote JVM Debug*
configuration to that port. Maven's own JVM is not debugged; the plugin API
cannot attach the IDE's Java debugger by itself.

**Reimport** runs the IDE's *Reload All Maven Projects*.

### Dependency Analyzer

Opening a `pom.xml` shows the analyzer beside the text (switch with *Editor
Only / Editor and Preview / Preview Only*):

- *Conflicts*, *All Dependencies as List*, *All Dependencies as Tree*, with
  search and GroupId / scope toggles.
- Select an artifact to see every path that brings it in.
- **Exclude from …** adds the `<exclusion>` to the direct dependency that
  brings the artifact in: one undoable edit, in the file's indentation.
- **Jump to …** opens that dependency's declaration.

The graph is the one the IDE resolved (`build.dependencies`, plugin API 1.10),
from the last **Reload** in the Maven tool window. After editing the POM,
reload Maven (or *Reimport*) and press **Refresh**.

**What it adds to the IDE's own Maven support:** the IDE has a Dependency
Analyzer and a Maven tool window. This plugin adds the one-click *Exclude* in
the POM, the per-artifact "paths to here" view next to the POM, Run / Debug
Maven goals from any file's context menu on its own module, and saved goals.

### Settings

Settings › Plugins › Maven Helper:

| Setting         | Default                            | Used by                                   |
| --------------- | ---------------------------------- | ----------------------------------------- |
| Goals           | clean, validate, … dependency:tree | *Saved Goals…* list, after your own goals |
| Extra arguments | (empty)                            | Every run, e.g. `-o` or `-T 1C`           |
| Debug port      | 5005                               | Debug Maven                               |

## Permissions

| Capability     | Risk   | Why                                                                                                                                                                                               |
| -------------- | ------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `process:mvn`  | High   | Runs Maven: `mvn` with the goals you choose, in the Run tool window or a log tab. Nothing else is started. Settings › Plugins can point the grant at another executable (a wrapper).              |
| `ide.commands` | High   | **Optional.** Only *Reimport* uses it, to run the IDE action `maven.reload` (Reload All Maven Projects). No other IDE action is called. Without it everything else works.                          |
| `editor.write` | Medium | The analyzer adds `<exclusion>` elements to the open `pom.xml` when you click *Exclude*, as one undoable edit.                                                                                    |
| `editor.read`  | Low    | The analyzer reads its `pom.xml` (module coordinates, where a dependency is declared).                                                                                                            |
| `project.read` | Low    | Finds the module `pom.xml` for the clicked file and reads module names.                                                                                                                           |
| `build.read`   | Low    | Reads the dependency graph the IDE already resolved. No build tool is run for it.                                                                                                                 |
| `storage`      | Low    | Keeps the last goal run (Re-run) and the saved goals.                                                                                                                                             |

No network access. The only file the plugin changes is the open `pom.xml`,
when you click *Exclude*.

### Run configuration markers

The IDE asks a plugin for run configurations only when a marker file is at
the project root. The plugin declares `pom.xml` and, for a folder of modules
without a root `pom.xml`, `.openidea/settings.toml` and
`.openidea/changelists.json` (files the IDE keeps per project). The
configurations only run `mvn`.

## Build

Needs Rust with the `wasm32-wasip2` target and the `openidea-plugin` CLI:

```sh
cargo install --locked --git https://github.com/l0ng-ai/openidea ide-host --bin openidea-plugin
```

```sh
cargo test               # logic, against the SDK's mock host
openidea-plugin build    # → dist/plugin.wasm, validated like an install
openidea-plugin link     # load this folder in the IDE, then Reload Plugins
```

Release: `scripts/release.sh <path-to-key>` builds, tests, signs and prints the
package's SHA-256; `publishing/` has the registry entries.

### Reproducible builds

Releases carry the component that the **Build** workflow compiled on GitHub
from the release tag, and the workflow checks every published release
against a fresh build. To rebuild it yourself, on Linux x86_64:

```sh
# Rust 1.100.0 with wasm32-wasip2 (rust-toolchain.toml), Cargo.lock as is
RUSTFLAGS="--remap-path-prefix=$HOME/.cargo=/cargo" \
  cargo build --release --locked --target wasm32-wasip2
sha256sum target/wasm32-wasip2/release/*.wasm   # = dist/plugin.wasm of the package
```

The cargo home path, which appears in panic messages, is the only
machine-specific input; the host matters too (a macOS build differs).
`scripts/release.sh <key>` tags the version, waits for the workflow, packs
and signs its artifact and creates the GitHub release.

| Path                   | What                                                              |
| ---------------------- | ----------------------------------------------------------------- |
| `plugin.toml`          | Manifest: custom editor, commands, menus, run configurations      |
| `src/lib.rs`           | Commands, run configurations, analyzer page requests              |
| `src/pom.rs`           | Minimal POM scanner: coordinates, dependency lookup, exclusions   |
| `web/`                 | The analyzer page (plain HTML/CSS/JS, no build step, no minifier) |
| `settings.schema.json` | Settings                                                          |
| `icons/`               | Plugin icon                                                       |

Requires OpenIDEA 0.1.47 or later (plugin API 1.10).

## License

Apache License 2.0, see [LICENSE](LICENSE) and [NOTICE](NOTICE).
