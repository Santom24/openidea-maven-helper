//! Maven Helper for OpenIDEA.
//!
//! - **Dependency Analyzer**: a custom editor on `pom.xml` (a web page in
//!   `web/`) showing the module's dependency graph as the IDE resolved it at the
//!   last Maven reload: conflicts, the full tree, a flat list with search, and
//!   for any artifact the paths that bring it in. From there an artifact can be
//!   excluded from the direct dependency that drags it in, or that declaration
//!   opened in the POM.
//! - **Run goals**: run a Maven goal on the module of the current file (or on
//!   the root module) in a terminal, re-run the last one, or type a custom one.

pub mod pom;

use openidea_plugin::details::ProcessLog;
use openidea_plugin::views::{View, ViewEvent, Viewport, Views};
use openidea_plugin::{
    DependencyNode, DependencyStatus, Error, Invocation, NotificationLevel, PickItem, Plugin,
    Project, Range, TextEdit, host, register_plugin, text,
};
use openidea_plugin::{PluginRunConfiguration, RunConfigurations};
use serde_json::{Value, json};

/// The Dependency Analyzer custom editor.
pub const VIEW: &str = "analyzer";
/// The `process:` program goals run with.
pub const MAVEN: &str = "mvn";
const DEFAULT_GOALS: &str = "clean, validate, compile, test, package, verify, install, deploy, clean install, dependency:tree";
const CUSTOM: &str = "\u{0}custom";
/// The goals of the *Run Maven* / *Debug Maven* submenus: command id suffix
/// (`run.<id>`, `debug.<id>`) and goals. plugin.toml declares the same list.
pub const MENU_GOALS: &[(&str, &str)] = &[
    ("clean", "clean"),
    ("validate", "validate"),
    ("compile", "compile"),
    ("test", "test"),
    ("package", "package"),
    ("verify", "verify"),
    ("install", "install"),
    ("clean-install", "clean install"),
    ("deploy", "deploy"),
    ("site", "site"),
    ("dependency-tree", "dependency:tree"),
];
const DEFAULT_DEBUG_PORT: u16 = 5005;
const LAST_RUN: &str = "last-run";
const SAVED_GOALS: &str = "saved-goals";
const MAX_SAVED_GOALS: usize = 50;
/// The IDE action that reloads every Maven project (Maven tool window › Reload).
pub const MAVEN_RELOAD: &str = "maven.reload";

#[derive(Default)]
pub struct MavenHelper;

// ---------------------------------------------------------------------------
// Dependency graph
// ---------------------------------------------------------------------------

fn status_name(s: DependencyStatus) -> &'static str {
    match s {
        DependencyStatus::Resolved => "resolved",
        DependencyStatus::OmittedForConflict => "conflict",
        DependencyStatus::OmittedForDuplicate => "duplicate",
        DependencyStatus::VersionManaged => "managed",
    }
}

/// The nodes under the root at `root` (itself first), with `parent` renumbered.
pub fn subtree(nodes: &[DependencyNode], root: usize) -> Vec<DependencyNode> {
    let mut index: Vec<Option<u32>> = vec![None; nodes.len()];
    let mut out: Vec<DependencyNode> = Vec::new();
    for (i, n) in nodes.iter().enumerate().skip(root) {
        let parent = if i == root {
            None
        } else {
            match n
                .parent
                .and_then(|p| index.get(p as usize).copied().flatten())
            {
                Some(p) => Some(p),
                None => continue,
            }
        };
        index[i] = Some(out.len() as u32);
        out.push(DependencyNode {
            parent,
            ..n.clone()
        });
    }
    out
}

/// The dependency graph of the module `coords` describes. Asked by
/// `groupId:artifactId`; when the IDE doesn't know that key (a group set by a
/// property, say), the module is looked up by artifact id among all of them.
pub fn module_graph(
    project: &Project,
    coords: &pom::Coordinates,
) -> Result<Vec<DependencyNode>, Error> {
    match project.build_dependencies(Some(&coords.key())) {
        Err(Error::NotFound(_)) => {}
        other => return other,
    }
    let all = project.build_dependencies(None)?;
    let roots: Vec<usize> = all
        .iter()
        .enumerate()
        .filter(|(_, n)| n.parent.is_none() && n.artifact == coords.artifact)
        .map(|(i, _)| i)
        .collect();
    let root = roots
        .iter()
        .copied()
        .find(|&i| all[i].group == coords.group)
        .or_else(|| (roots.len() == 1).then(|| roots[0]))
        .ok_or_else(|| {
            Error::NotFound(format!(
                "Module {} is not in the IDE's Maven model. Open the Maven tool window and Reload.",
                coords.key()
            ))
        })?;
    Ok(subtree(&all, root))
}

/// The graph as the page reads it.
pub fn graph_json(coords: &pom::Coordinates, nodes: &[DependencyNode]) -> Value {
    let nodes: Vec<Value> = nodes
        .iter()
        .map(|n| {
            json!({
                "g": n.group,
                "a": n.artifact,
                "v": n.version,
                "s": n.scope,
                "st": status_name(n.status),
                "rv": n.requested_version,
                "p": n.parent,
            })
        })
        .collect();
    json!({ "module": coords.key(), "version": coords.version, "nodes": nodes })
}

// ---------------------------------------------------------------------------
// Analyzer page messages
// ---------------------------------------------------------------------------

fn str_field<'a>(v: &'a Value, path: &[&str]) -> Result<&'a str, Error> {
    let mut cur = v;
    for key in path {
        cur = cur
            .get(key)
            .ok_or_else(|| Error::Invalid(format!("missing {}", path.join("."))))?;
    }
    cur.as_str()
        .ok_or_else(|| Error::Invalid(format!("{} is not a string", path.join("."))))
}

impl MavenHelper {
    /// Answers one request of the page; the reply carries its `nonce` so that
    /// the analyzer of another POM ignores it.
    fn answer(&mut self, msg: &Value, ctx: &Invocation) -> Result<Value, Error> {
        let editor = ctx
            .editor()
            .ok_or_else(|| Error::Invalid("the analyzer needs its pom.xml".into()))?;
        let doc = editor.document()?;
        match msg.get("op").and_then(Value::as_str) {
            Some("load") => {
                let coords = pom::coordinates(&doc.text).ok_or_else(|| {
                    Error::Invalid("This file has no <project>/<artifactId>.".into())
                })?;
                let project = ctx
                    .project()
                    .ok_or_else(|| Error::Invalid("Open the project first.".into()))?;
                let nodes = module_graph(&project, &coords)?;
                let mut out = graph_json(&coords, &nodes);
                out["op"] = "graph".into();
                Ok(out)
            }
            Some("exclude") => {
                let (g, a) = (
                    str_field(msg, &["dep", "g"])?,
                    str_field(msg, &["dep", "a"])?,
                );
                let (xg, xa) = (
                    str_field(msg, &["target", "g"])?,
                    str_field(msg, &["target", "a"])?,
                );
                let ins = pom::exclusion_edit(&doc.text, g, a, xg, xa).map_err(|e| match e {
                    pom::ExcludeError::NotDeclared(d) => Error::Invalid(format!(
                        "{d} is not declared in this POM's <dependencies> (inherited from a parent or a profile?)."
                    )),
                    pom::ExcludeError::AlreadyExcluded => {
                        Error::Invalid(format!("{xg}:{xa} is already excluded from {g}:{a}."))
                    }
                })?;
                let at = text::position(&doc.text, ins.at);
                editor.apply_edits(
                    doc.version,
                    &[TextEdit {
                        range: Range::new(at, at),
                        text: ins.text,
                    }],
                )?;
                Ok(json!({
                    "op": "done",
                    "message": format!("Excluded {xg}:{xa} from {g}:{a}. Reload the Maven project to refresh the graph."),
                }))
            }
            Some("open") => {
                let (g, a) = (str_field(msg, &["g"])?, str_field(msg, &["a"])?);
                let dep = pom::find_dependency(&doc.text, g, a).ok_or_else(|| {
                    Error::Invalid(format!(
                        "{g}:{a} is not declared in this POM's <dependencies>."
                    ))
                })?;
                let at = text::position(&doc.text, dep.start);
                host::open_location(&editor.path(), at.line, at.character)?;
                Ok(json!({ "op": "done" }))
            }
            other => Err(Error::Invalid(format!("unknown request {other:?}"))),
        }
    }
}

impl Views for MavenHelper {
    fn render(
        &mut self,
        view: &str,
        _viewport: Option<Viewport>,
        _ctx: &Invocation,
    ) -> Result<View, Error> {
        if view != VIEW {
            return Err(Error::NotFound(view.into()));
        }
        Ok(View::message(
            "The Dependency Analyzer is shown by the plugin's page.",
        ))
    }

    fn handle(&mut self, view: &str, event: ViewEvent, ctx: &Invocation) -> Result<(), Error> {
        if view != VIEW {
            return Err(Error::NotFound(view.into()));
        }
        let ViewEvent::Message(message) = event else {
            return Ok(());
        };
        let Ok(msg) = serde_json::from_str::<Value>(&message) else {
            return Ok(());
        };
        let mut reply = self
            .answer(&msg, ctx)
            .unwrap_or_else(|e| json!({ "op": "error", "message": e.to_string() }));
        reply["nonce"] = msg.get("nonce").cloned().unwrap_or(Value::Null);
        host::post_view_message(VIEW, &reply.to_string())
    }
}

// ---------------------------------------------------------------------------
// Running goals
// ---------------------------------------------------------------------------

fn parent_dir(path: &str) -> Option<&str> {
    let cut = path.trim_end_matches(['/', '\\']).rfind(['/', '\\'])?;
    Some(if cut == 0 { &path[..1] } else { &path[..cut] })
}

fn join(dir: &str, name: &str) -> String {
    format!("{}/{}", dir.trim_end_matches(['/', '\\']), name)
}

/// The `pom.xml` of the module containing `path`: the nearest one at or above
/// it, inside `root`.
pub fn nearest_pom(project: &Project, root: &str, path: &str) -> Option<String> {
    if path.ends_with("pom.xml") && project.exists(path).unwrap_or(false) {
        return Some(path.to_owned());
    }
    let mut dir = Some(path);
    while let Some(d) = dir {
        if !d.starts_with(root) {
            break;
        }
        let candidate = join(d, "pom.xml");
        if project.exists(&candidate).unwrap_or(false) {
            return Some(candidate);
        }
        if d.trim_end_matches(['/', '\\']) == root.trim_end_matches(['/', '\\']) {
            break;
        }
        dir = parent_dir(d);
    }
    None
}

/// The topmost `pom.xml` at or above `path` inside `root`: the root of the
/// multi-module build `path` belongs to.
pub fn topmost_pom(project: &Project, root: &str, path: &str) -> Option<String> {
    let mut best = nearest_pom(project, root, path)?;
    while let Some(above) = parent_dir(&best)
        .and_then(parent_dir)
        .and_then(|d| nearest_pom(project, root, d))
    {
        if above == best {
            break;
        }
        best = above;
    }
    Some(best)
}

/// The goals offered in the picker, from the `goals` setting.
pub fn configured_goals() -> Vec<String> {
    let raw = host::setting_string("goals")
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_GOALS.into());
    raw.split([',', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Goals passed as the command's argument, if it holds goals: menus pass the
/// clicked path (or a problem's JSON location) instead.
pub fn goals_arg(arg: Option<&str>, ctx: &Invocation) -> Option<String> {
    let a = arg?.trim();
    let is_path = a.starts_with(['/', '\\', '{', '~'])
        || a.get(1..3) == Some(":\\")
        || a.get(1..3) == Some(":/")
        || ctx.selected_paths().iter().any(|p| p == a);
    (!a.is_empty() && !is_path).then(|| a.to_owned())
}

/// Goals added with *New Goal…*, most recent first.
pub fn saved_goals() -> Result<Vec<String>, Error> {
    Ok(host::storage::get(SAVED_GOALS)?
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_default())
}

/// `mvn` arguments for `goals` on `pom`, with the `extraArgs` setting.
pub fn maven_args(pom: &str, goals: &str) -> Vec<String> {
    let mut args = vec!["-f".to_owned(), pom.to_owned()];
    args.extend(
        host::setting_string("extraArgs")
            .unwrap_or_default()
            .split_whitespace()
            .map(str::to_owned),
    );
    args.extend(goals.split_whitespace().map(str::to_owned));
    args
}

impl MavenHelper {
    /// The POM to run on: the module of what the command was run on (the
    /// Project view or tab selection, else the focused editor); `root_module`
    /// takes the topmost POM above it instead. Without one (a folder holding
    /// several modules, a file outside any), the user picks a module.
    fn module_pom(
        &self,
        ctx: &Invocation,
        root_module: bool,
    ) -> Result<Option<(Project, String)>, Error> {
        let project = ctx.project().ok_or("Open a Maven project first.")?;
        let root = project.root()?;
        let from = ctx
            .selected_paths()
            .into_iter()
            .next()
            .or_else(|| ctx.editor().map(|e| e.path()));
        let found = from.and_then(|p| {
            if root_module {
                topmost_pom(&project, &root, &p)
            } else {
                nearest_pom(&project, &root, &p)
            }
        });
        if let Some(pom) = found {
            return Ok(Some((project, pom)));
        }
        let root_pom = join(&root, "pom.xml");
        if project.exists(&root_pom)? {
            return Ok(Some((project, root_pom)));
        }
        let mut poms: Vec<String> = project
            .find_files("**/pom.xml", 500)?
            .into_iter()
            .filter(|p| !p.contains("/target/") && !p.contains("/src/"))
            .collect();
        poms.sort_by_key(|p| (p.matches('/').count(), p.clone()));
        if root_module {
            // Only the modules no other POM contains.
            let all = poms.clone();
            poms.retain(|p| {
                !all.iter().any(|q| {
                    q != p && parent_dir(q).is_some_and(|d| p.starts_with(&format!("{d}/")))
                })
            });
        }
        let pom = match poms.len() {
            0 => return Err("No pom.xml found in this project.".into()),
            1 => poms.pop(),
            _ => {
                let items: Vec<PickItem> = poms
                    .iter()
                    .map(|p| {
                        let rel = p
                            .strip_prefix(&root)
                            .unwrap_or(p)
                            .trim_start_matches(['/', '\\']);
                        let name = project
                            .read_file(p)
                            .ok()
                            .and_then(|t| pom::coordinates(&t))
                            .map(|c| c.artifact);
                        PickItem::new(p, name.as_deref().unwrap_or(rel)).description(rel)
                    })
                    .collect();
                host::pick("Maven Module", &items)?
            }
        };
        Ok(pom.map(|p| (project, p)))
    }

    fn choose_goals(&self, debug: bool) -> Result<Option<String>, Error> {
        let saved = saved_goals()?;
        let mut items: Vec<PickItem> = saved
            .iter()
            .map(|g| PickItem::new(g, g).description("saved"))
            .collect();
        items.extend(
            configured_goals()
                .iter()
                .filter(|g| !saved.contains(g))
                .map(|g| PickItem::new(g, g)),
        );
        items.push(
            PickItem::new(CUSTOM, "New Goal…")
                .description("type goals and options, saved for next time"),
        );
        match host::pick(
            if debug {
                "Debug Maven Goal"
            } else {
                "Run Maven Goal"
            },
            &items,
        )? {
            Some(k) if k == CUSTOM => self.new_goal(),
            other => Ok(other),
        }
    }

    /// Asks for goals and keeps them in the saved list (most recent first).
    fn new_goal(&self) -> Result<Option<String>, Error> {
        let Some(goals) = host::input("New Maven goal (e.g. clean install -DskipTests)", None)?
            .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|s| !s.is_empty())
        else {
            return Ok(None);
        };
        let mut saved = saved_goals()?;
        saved.retain(|g| *g != goals);
        saved.insert(0, goals.clone());
        saved.truncate(MAX_SAVED_GOALS);
        host::storage::set(
            SAVED_GOALS,
            &serde_json::to_string(&saved).unwrap_or_default(),
        )?;
        Ok(Some(goals))
    }

    fn input_goals(&self) -> Result<Option<String>, Error> {
        let last = host::storage::get(LAST_RUN)?
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .and_then(|v| v.get("goals").and_then(Value::as_str).map(str::to_owned));
        Ok(host::input(
            "Maven goals and options (e.g. clean install -DskipTests)",
            last.as_deref(),
        )?
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty()))
    }

    fn run(
        &self,
        project: &Project,
        pom: &str,
        goals: &str,
        debug: bool,
    ) -> Result<Option<String>, Error> {
        let port = debug.then(debug_port);
        // The Run tool window (Stop, Rerun, output kept) through the plugin's
        // run configurations; the log tab when the IDE hasn't asked for them
        // (no marker file at the project root) or can't start one.
        let (config, args) = run_request(pom, goals, port);
        let mut started = host::start_run(&config, &args, false);
        if matches!(started, Err(Error::NotFound(_))) && config != GENERIC_CONFIG {
            // A goal saved since the IDE last asked for the configurations.
            let (_, mut generic) = run_request(pom, "", port);
            generic.splice(2..2, goals.split_whitespace().map(str::to_owned));
            started = host::start_run(GENERIC_CONFIG, &generic, false);
        }
        match started {
            Ok(_) => {}
            Err(Error::NotFound(_) | Error::Invalid(_) | Error::Failed(_)) => {
                let (key, title, log) = log_tab_for(project, pom, goals, port);
                host::open_log_tab(&key, &title, &log)?;
            }
            Err(e) => return Err(e),
        }
        host::storage::set(
            LAST_RUN,
            &json!({ "pom": pom, "goals": goals, "debug": debug }).to_string(),
        )?;
        if let Some(port) = port {
            host::notify(
                NotificationLevel::Info,
                "Debug Maven",
                &format!(
                    "mvn {goals}: the JVMs it starts for tests (Surefire, Failsafe) and spring-boot:run wait for a debugger on \
                     localhost:{port}. Attach with a Remote JVM Debug configuration on port {port}."
                ),
            );
        }
        Ok(None)
    }
}

/// Configuration id of the generic run (any goals, appended).
pub const GENERIC_CONFIG: &str = "mvn";

/// The run configurations the Run tool window starts: one per menu goal
/// (`Maven: clean install`) so its tab names the goal, and a generic `Maven`
/// for the others. The module and options are appended when started.
pub fn run_configurations() -> Vec<PluginRunConfiguration> {
    let mut out: Vec<PluginRunConfiguration> = MENU_GOALS
        .iter()
        .map(|(id, goals)| PluginRunConfiguration {
            id: format!("goal.{id}"),
            name: format!("Maven: {goals}"),
            program: MAVEN.into(),
            args: goals.split_whitespace().map(str::to_owned).collect(),
            ..Default::default()
        })
        .collect();
    // Goals added with New Goal…: they show in the IDE's run configuration
    // selector, and their Run tab is named after them.
    let menu: Vec<&str> = MENU_GOALS.iter().map(|(_, g)| *g).collect();
    out.extend(
        saved_goals()
            .unwrap_or_default()
            .into_iter()
            .filter(|g| !menu.contains(&g.as_str()))
            .map(|g| PluginRunConfiguration {
                id: saved_config_id(&g),
                name: format!("Maven: {g}"),
                program: MAVEN.into(),
                args: g.split_whitespace().map(str::to_owned).collect(),
                ..Default::default()
            }),
    );
    out.push(PluginRunConfiguration {
        id: GENERIC_CONFIG.into(),
        name: "Maven".into(),
        program: MAVEN.into(),
        ..Default::default()
    });
    out
}

/// A stable configuration id for a saved goal (FNV-1a of the goals).
pub fn saved_config_id(goals: &str) -> String {
    let h = goals.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    format!("saved.{h:016x}")
}

/// The configuration and appended arguments that run `goals` on `pom`.
pub fn run_request(pom: &str, goals: &str, debug_port: Option<u16>) -> (String, Vec<String>) {
    let menu = MENU_GOALS.iter().find(|(_, g)| *g == goals);
    let saved = menu.is_none() && saved_goals().unwrap_or_default().iter().any(|g| g == goals);
    let (config, mut args) = match menu {
        // mvn <goals> -f <pom> [extra]: Maven takes options after goals too.
        Some((id, _)) => (format!("goal.{id}"), maven_args(pom, "")),
        None if saved => (saved_config_id(goals), maven_args(pom, "")),
        None => (GENERIC_CONFIG.to_owned(), maven_args(pom, goals)),
    };
    // The Run window reads Maven through a pipe, where Maven turns colours
    // off; its console renders ANSI colours, so ask for them.
    args.push("-Dstyle.color=always".into());
    if let Some(port) = debug_port {
        args.extend(debug_args(port));
    }
    (config, args)
}

/// The JDWP port Debug Maven's JVMs listen on (`debugPort` setting).
pub fn debug_port() -> u16 {
    host::setting_number("debugPort")
        .filter(|p| (1.0..=65535.0).contains(p))
        .map_or(DEFAULT_DEBUG_PORT, |p| p as u16)
}

/// Arguments that make the JVMs Maven forks wait for a debugger on `port`:
/// tests (Surefire, Failsafe) and `spring-boot:run`. Only arguments, so they
/// need no environment and leave Maven's own JVM alone.
pub fn debug_args(port: u16) -> Vec<String> {
    let jdwp = format!("-agentlib:jdwp=transport=dt_socket,server=y,suspend=y,address={port}");
    vec![
        format!("-Dmaven.surefire.debug={jdwp}"),
        format!("-Dmaven.failsafe.debug={jdwp}"),
        format!("-Dspring-boot.run.jvmArguments={jdwp}"),
    ]
}

/// The log tab that runs `goals` on `pom`: a fresh key per run (the IDE would
/// only focus a finished tab with the same key), its title and the process.
/// The tab stays open with the output and exit code when Maven ends; its
/// toggles re-run with -o / -DskipTests / -U.
pub fn log_tab_for(
    project: &Project,
    pom: &str,
    goals: &str,
    debug_port: Option<u16>,
) -> (String, String, ProcessLog) {
    let module = project
        .read_file(pom)
        .ok()
        .and_then(|t| pom::coordinates(&t))
        .map(|c| c.artifact)
        .unwrap_or_else(|| "pom.xml".into());
    let key = format!("mvn-{}", next_run_id());
    let mut title = match debug_port {
        Some(port) => format!("Debug mvn {goals} [{module}] :{port}"),
        None => format!("mvn {goals} [{module}]"),
    };
    if title.len() > 200 {
        let cut = (0..=200)
            .rev()
            .find(|&i| title.is_char_boundary(i))
            .unwrap_or(0);
        title.truncate(cut);
        title.push('…');
    }
    let mut args = maven_args(pom, goals);
    if let Some(port) = debug_port {
        args.extend(debug_args(port));
    }
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let log = ProcessLog::new(MAVEN, &args)
        .toggle("offline", "Offline", &["-o"])
        .toggle("skip-tests", "Skip tests", &["-DskipTests"])
        .toggle("update", "Update snapshots", &["-U"]);
    (key, title, log)
}

fn next_run_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    nanos.wrapping_add(SEQ.fetch_add(1, Ordering::Relaxed))
}

/// Runs the IDE's Reload All Maven Projects. It answers only when Maven has
/// finished (tens of seconds): an open task lets this call run 60 s instead of 5.
fn reload_maven() -> Result<Option<String>, Error> {
    let _task = host::begin_task("Reimporting Maven projects", false);
    host::execute_command(MAVEN_RELOAD, None)
}

impl RunConfigurations for MavenHelper {
    fn configurations(&mut self, _project_root: &str) -> Vec<PluginRunConfiguration> {
        run_configurations()
    }
}

impl Plugin for MavenHelper {
    fn run_command(
        &mut self,
        id: &str,
        ctx: &Invocation,
        arg: Option<&str>,
    ) -> Result<Option<String>, Error> {
        let (debug, name) = match id.split_once('.') {
            Some(("run", rest)) => (false, rest),
            Some(("debug", rest)) => (true, rest),
            _ => (id.starts_with("debug-"), id),
        };
        // A fixed goal of the Run Maven / Debug Maven submenus.
        if let Some((_, goals)) = MENU_GOALS.iter().find(|(k, _)| *k == name) {
            return match self.module_pom(ctx, false)? {
                Some((project, pom)) => self.run(&project, &pom, goals, debug),
                None => Ok(None),
            };
        }
        match id {
            "run-goal" | "run-goal-root" | "run-custom-goal" | "debug-goal" | "debug-custom-goal"
            | "new-goal" | "debug-new-goal" => {
                let Some((project, pom)) = self.module_pom(ctx, id == "run-goal-root")? else {
                    return Ok(None);
                };
                // Another feature may pass the goals directly; from a menu the
                // argument is the clicked file instead.
                let goals = match goals_arg(arg, ctx) {
                    Some(a) => Some(a.to_owned()),
                    None if id.ends_with("custom-goal") => self.input_goals()?,
                    None if id.ends_with("new-goal") => self.new_goal()?,
                    None => self.choose_goals(debug)?,
                };
                match goals {
                    Some(g) => self.run(&project, &pom, &g, debug),
                    None => Ok(None),
                }
            }
            // The IDE's reload answers only when Maven has finished (tens of
            // seconds): an open task lets this call run 60 s instead of 5.
            "reimport" => match reload_maven() {
                Err(Error::Denied(_)) => Err(Error::Invalid(
                    "Reimport runs the IDE's Reload All Maven Projects, which needs the optional \"ide.commands\" permission: Settings › Plugins › Maven Helper › Permissions, tick ide.commands. Meanwhile: Maven tool window › Reload.".into(),
                )),
                other => other.map(|_| None),
            },
            "remove-goal" => {
                let mut saved = saved_goals()?;
                if saved.is_empty() {
                    return Err("No saved goals: add one with New Goal….".into());
                }
                let items: Vec<PickItem> = saved.iter().map(|g| PickItem::new(g, g)).collect();
                let Some(gone) = host::pick_many("Remove Saved Goals", &items)?.filter(|g| !g.is_empty()) else {
                    return Ok(None);
                };
                saved.retain(|g| !gone.contains(g));
                host::storage::set(SAVED_GOALS, &serde_json::to_string(&saved).unwrap_or_default())?;
                let n = gone.len();
                Ok(Some(format!("Removed {n} saved goal{}.", if n == 1 { "" } else { "s" })))
            }
            "rerun-last" => {
                let last = host::storage::get(LAST_RUN)?
                    .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                    .ok_or("No Maven goal has been run yet.")?;
                let pom = str_field(&last, &["pom"])?.to_owned();
                let goals = str_field(&last, &["goals"])?.to_owned();
                let debug = last.get("debug").and_then(Value::as_bool).unwrap_or(false);
                let project = host::project_of(&pom)
                    .or_else(|| ctx.project())
                    .ok_or("Open the project first.")?;
                self.run(&project, &pom, &goals, debug)
            }
            _ => Err(Error::NotFound(format!("command {id}"))),
        }
    }
}

register_plugin!(MavenHelper, views, run_configurations);

#[cfg(test)]
mod tests {
    use super::*;
    use openidea_plugin::mock::MockHost;

    const CAPS: &[&str] = &[
        "editor.read",
        "editor.write",
        "project.read",
        "build.read",
        "storage",
        "process:mvn",
    ];

    fn node(
        g: &str,
        a: &str,
        v: &str,
        st: DependencyStatus,
        rv: Option<&str>,
        parent: Option<u32>,
    ) -> DependencyNode {
        DependencyNode {
            group: g.into(),
            artifact: a.into(),
            version: v.into(),
            scope: if parent.is_some() {
                "compile".into()
            } else {
                String::new()
            },
            status: st,
            requested_version: rv.map(Into::into),
            parent,
        }
    }

    fn graph() -> Vec<DependencyNode> {
        use DependencyStatus::*;
        vec![
            node("com.acme", "lib", "1.0", Resolved, None, None), // 0: another module
            node("x", "y", "1", Resolved, None, Some(0)),
            node("com.acme", "app", "1.0", Resolved, None, None), // 2
            node(
                "org.springframework",
                "spring-core",
                "6.1.0",
                Resolved,
                None,
                Some(2),
            ),
            node(
                "commons-logging",
                "commons-logging",
                "1.2",
                Resolved,
                None,
                Some(3),
            ),
            node("org.other", "thing", "2.0", Resolved, None, Some(2)),
            node(
                "commons-logging",
                "commons-logging",
                "1.2",
                OmittedForConflict,
                Some("1.1"),
                Some(5),
            ),
        ]
    }

    const POM: &str = "<project>\n  <groupId>com.acme</groupId>\n  <artifactId>app</artifactId>\n  <version>1.0</version>\n  <dependencies>\n    <dependency>\n      <groupId>org.other</groupId>\n      <artifactId>thing</artifactId>\n    </dependency>\n  </dependencies>\n</project>\n";

    fn send(host: &MockHost, msg: &str) -> Value {
        MavenHelper
            .handle(VIEW, ViewEvent::Message(msg.into()), &host.invocation())
            .unwrap();
        let (view, reply) = host.view_messages().last().cloned().unwrap();
        assert_eq!(view, VIEW);
        serde_json::from_str(&reply).unwrap()
    }

    #[test]
    fn subtree_keeps_one_module() {
        let sub = subtree(&graph(), 2);
        assert_eq!(sub.len(), 5);
        assert_eq!(sub[0].artifact, "app");
        assert_eq!(sub[2].parent, Some(1), "commons-logging under spring-core");
        assert_eq!(sub[4].parent, Some(3));
    }

    #[test]
    fn page_loads_the_module_graph() {
        let host = MockHost::install().with_capabilities(CAPS);
        host.set_build_dependencies(graph());
        host.open_editor("/project/pom.xml", "xml", POM, &[]);
        let r = send(&host, r#"{"op":"load","nonce":"n1"}"#);
        assert_eq!(r["op"], "graph");
        assert_eq!(r["nonce"], "n1");
        assert_eq!(r["module"], "com.acme:app");
        let nodes = r["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 5);
        assert_eq!(nodes[4]["st"], "conflict");
        assert_eq!(nodes[4]["rv"], "1.1");
    }

    #[test]
    fn falls_back_to_artifact_id_when_group_is_a_property() {
        let host = MockHost::install().with_capabilities(CAPS);
        host.set_build_dependencies(graph());
        host.open_editor(
            "/project/pom.xml",
            "xml",
            &POM.replace("com.acme</groupId>", "${g}</groupId>"),
            &[],
        );
        let r = send(&host, r#"{"op":"load","nonce":1}"#);
        assert_eq!(r["op"], "graph", "{r}");
        assert_eq!(r["nodes"].as_array().unwrap().len(), 5);
    }

    #[test]
    fn unknown_module_is_reported_to_the_page() {
        let host = MockHost::install().with_capabilities(CAPS);
        host.set_build_dependencies(graph());
        host.open_editor(
            "/project/pom.xml",
            "xml",
            &POM.replace(">app<", ">nope<"),
            &[],
        );
        let r = send(&host, r#"{"op":"load","nonce":2}"#);
        assert_eq!(r["op"], "error");
        assert!(r["message"].as_str().unwrap().contains("Reload"));
    }

    #[test]
    fn excludes_through_the_direct_dependency() {
        let host = MockHost::install().with_capabilities(CAPS);
        host.open_editor("/project/pom.xml", "xml", POM, &[]);
        let r = send(
            &host,
            r#"{"op":"exclude","nonce":3,"dep":{"g":"org.other","a":"thing"},"target":{"g":"commons-logging","a":"commons-logging"}}"#,
        );
        assert_eq!(r["op"], "done", "{r}");
        assert!(host.editor_text().contains(
            "      <artifactId>thing</artifactId>\n      <exclusions>\n        <exclusion>\n          <groupId>commons-logging</groupId>"
        ));
        let r = send(
            &host,
            r#"{"op":"exclude","nonce":4,"dep":{"g":"org.other","a":"thing"},"target":{"g":"commons-logging","a":"commons-logging"}}"#,
        );
        assert_eq!(r["op"], "error");
    }

    #[test]
    fn opens_the_declaration() {
        let host = MockHost::install().with_capabilities(CAPS);
        host.open_editor("/project/pom.xml", "xml", POM, &[]);
        let r = send(
            &host,
            r#"{"op":"open","nonce":5,"g":"org.other","a":"thing"}"#,
        );
        assert_eq!(r["op"], "done", "{r}");
        assert_eq!(
            host.opened_locations(),
            vec![("/project/pom.xml".to_owned(), 5, 4)]
        );
    }

    /// The program and arguments of the last log tab opened, and its title.
    fn last_run(host: &MockHost) -> (String, Vec<String>) {
        let tab = host.log_tabs().last().cloned().expect("a log tab");
        assert_eq!(tab.log.program, "mvn");
        (tab.title, tab.log.args)
    }

    #[test]
    fn runs_in_the_run_tool_window_when_the_ide_has_the_configurations() {
        let host = MockHost::install().with_capabilities(CAPS);
        host.set_run_configurations(MavenHelper.configurations("/project"));
        host.set_project_root(Some("/project"));
        host.add_file("/project/pom.xml", POM);
        host.select_paths(&["/project/pom.xml"]);

        MavenHelper
            .run_command("run.clean-install", &host.invocation(), None)
            .unwrap();
        MavenHelper
            .run_command("debug.test", &host.invocation(), None)
            .unwrap();
        host.answer_next_prompt(Some(&[CUSTOM]));
        host.answer_next_prompt(Some(&["verify -Pquality"]));
        MavenHelper
            .run_command("run-goal", &host.invocation(), None)
            .unwrap();

        let runs = host.configuration_runs();
        assert_eq!(
            (runs[0].1.as_str(), runs[0].2.clone()),
            (
                "goal.clean-install",
                vec![
                    "-f".to_owned(),
                    "/project/pom.xml".into(),
                    "-Dstyle.color=always".into()
                ]
            )
        );
        assert_eq!(runs[1].1, "goal.test");
        assert!(runs[1].2.iter().any(|a| a.ends_with("address=5005")));
        assert_eq!(runs[2].1, GENERIC_CONFIG);
        assert_eq!(
            runs[2].2,
            [
                "-f",
                "/project/pom.xml",
                "verify",
                "-Pquality",
                "-Dstyle.color=always"
            ]
        );
        assert!(host.log_tabs().is_empty(), "no fallback needed");

        let configs = run_configurations();
        assert_eq!(
            configs.len(),
            MENU_GOALS.len() + 2,
            "menu goals, the saved one, the generic one"
        );
        let ci = configs
            .iter()
            .find(|c| c.id == "goal.clean-install")
            .unwrap();
        assert_eq!(
            (ci.name.as_str(), ci.args.clone()),
            (
                "Maven: clean install",
                vec!["clean".to_owned(), "install".into()]
            )
        );
    }

    #[test]
    fn saved_goals_become_run_configurations() {
        let host = MockHost::install().with_capabilities(CAPS);
        host.set_project_root(Some("/project"));
        host.add_file("/project/pom.xml", POM);
        host.select_paths(&["/project/pom.xml"]);
        host.set_run_configurations(MavenHelper.configurations("/project"));

        // Saved before the IDE asked again: runs through the generic configuration.
        host.answer_next_prompt(Some(&["clean install -DskipTests"]));
        MavenHelper
            .run_command("new-goal", &host.invocation(), None)
            .unwrap();
        let runs = host.configuration_runs();
        assert_eq!(runs[0].1, GENERIC_CONFIG);
        assert_eq!(
            runs[0].2[..5],
            ["-f", "/project/pom.xml", "clean", "install", "-DskipTests"]
        );

        // Once asked again, it has its own configuration, named after it.
        let configs = MavenHelper.configurations("/project");
        let saved = configs
            .iter()
            .find(|c| c.id == saved_config_id("clean install -DskipTests"))
            .unwrap();
        assert_eq!(saved.name, "Maven: clean install -DskipTests");
        host.set_run_configurations(configs);
        host.answer_next_prompt(Some(&["clean install -DskipTests"]));
        MavenHelper
            .run_command("run-goal", &host.invocation(), None)
            .unwrap();
        let runs = host.configuration_runs();
        assert_eq!(runs[1].1, saved_config_id("clean install -DskipTests"));
        assert_eq!(runs[1].2[..2], ["-f", "/project/pom.xml"]);
        assert!(
            saved_config_id("a").starts_with("saved.")
                && saved_config_id("a") != saved_config_id("b")
        );
    }

    #[test]
    fn runs_a_goal_on_the_module_of_the_current_file() {
        let host = MockHost::install().with_capabilities(CAPS);
        host.set_project_root(Some("/project"));
        host.add_file("/project/pom.xml", POM);
        host.add_file("/project/core/pom.xml", &POM.replace(">app<", ">core<"));
        host.add_file("/project/core/src/main/java/A.java", "class A {}");
        host.open_editor(
            "/project/core/src/main/java/A.java",
            "java",
            "class A {}",
            &[],
        );

        host.answer_next_prompt(Some(&["clean install"]));
        MavenHelper
            .run_command("run-goal", &host.invocation(), None)
            .unwrap();
        assert_eq!(
            last_run(&host),
            (
                "mvn clean install [core]".into(),
                vec![
                    "-f".into(),
                    "/project/core/pom.xml".into(),
                    "clean".into(),
                    "install".into()
                ]
            )
        );
        let toggles: Vec<_> = host.log_tabs()[0]
            .log
            .toggles
            .iter()
            .map(|t| t.args.join(" "))
            .collect();
        assert_eq!(toggles, ["-o", "-DskipTests", "-U"]);

        MavenHelper
            .run_command("run-goal-root", &host.invocation(), Some("verify"))
            .unwrap();
        assert_eq!(last_run(&host).1, ["-f", "/project/pom.xml", "verify"]);

        // Re-run opens a new tab (a finished tab with the same key would only be focused).
        MavenHelper
            .run_command("rerun-last", &host.invocation(), None)
            .unwrap();
        let tabs = host.log_tabs();
        assert_eq!(tabs.len(), 3);
        assert_ne!(tabs[1].key, tabs[2].key);
        assert_eq!(tabs[2].log.args, ["-f", "/project/pom.xml", "verify"]);

        host.answer_next_prompt(Some(&[CUSTOM]));
        host.answer_next_prompt(Some(&["package -DskipTests"]));
        MavenHelper
            .run_command("run-goal", &host.invocation(), None)
            .unwrap();
        assert_eq!(
            last_run(&host).1,
            ["-f", "/project/core/pom.xml", "package", "-DskipTests"]
        );
    }

    #[test]
    fn menu_goals_debug_and_multi_module_roots() {
        let host = MockHost::install().with_capabilities(CAPS);
        // Like a folder of independent modules: no pom.xml at the root.
        host.set_project_root(Some("/ws"));
        host.add_file("/ws/a/pom.xml", &POM.replace(">app<", ">a<"));
        host.add_file("/ws/b/pom.xml", &POM.replace(">app<", ">b<"));
        host.add_file("/ws/b/core/pom.xml", &POM.replace(">app<", ">b-core<"));
        host.add_file("/ws/b/core/src/X.java", "class X {}");

        // Project view: right-click on a file of b/core runs on b/core; the
        // menu passes the clicked path as the argument, which is not a goal.
        host.select_paths(&["/ws/b/core/src/X.java"]);
        MavenHelper
            .run_command(
                "run.clean-install",
                &host.invocation(),
                Some("/ws/b/core/src/X.java"),
            )
            .unwrap();
        assert_eq!(
            last_run(&host).1,
            ["-f", "/ws/b/core/pom.xml", "clean", "install"]
        );

        // Its root module is b.
        MavenHelper
            .run_command("run-goal-root", &host.invocation(), Some("verify"))
            .unwrap();
        assert_eq!(last_run(&host).1, ["-f", "/ws/b/pom.xml", "verify"]);

        // Debug: the forked JVMs wait for a debugger; no environment needed.
        MavenHelper
            .run_command("debug.test", &host.invocation(), None)
            .unwrap();
        let (title, args) = last_run(&host);
        assert_eq!(title, "Debug mvn test [b-core] :5005");
        assert_eq!(args[..3], ["-f", "/ws/b/core/pom.xml", "test"]);
        assert!(args.contains(&"-Dmaven.surefire.debug=-agentlib:jdwp=transport=dt_socket,server=y,suspend=y,address=5005".into()));
        assert!(
            args.iter()
                .any(|a| a.starts_with("-Dspring-boot.run.jvmArguments="))
        );
        assert!(
            host.notifications()
                .last()
                .unwrap()
                .body
                .contains("localhost:5005")
        );
        MavenHelper
            .run_command("rerun-last", &host.invocation(), None)
            .unwrap();
        assert!(
            last_run(&host).0.starts_with("Debug "),
            "re-run keeps debug"
        );

        host.set_setting("debugPort", "5010");
        MavenHelper
            .run_command("debug.verify", &host.invocation(), None)
            .unwrap();
        assert!(
            last_run(&host)
                .1
                .iter()
                .any(|a| a.ends_with("address=5010"))
        );

        // The root folder itself has no POM: pick a module.
        host.select_paths(&["/ws"]);
        host.answer_next_prompt(Some(&["/ws/a/pom.xml"]));
        MavenHelper
            .run_command("run.install", &host.invocation(), Some("/ws"))
            .unwrap();
        assert_eq!(last_run(&host).1, ["-f", "/ws/a/pom.xml", "install"]);
        // Dismissed picker: nothing runs.
        let before = host.log_tabs().len();
        host.answer_next_prompt(None);
        MavenHelper
            .run_command("run.install", &host.invocation(), None)
            .unwrap();
        assert_eq!(host.log_tabs().len(), before);
    }

    #[test]
    fn new_goals_are_saved_and_offered_first() {
        let host = MockHost::install().with_capabilities(CAPS);
        host.set_project_root(Some("/project"));
        host.add_file("/project/pom.xml", POM);
        host.select_paths(&["/project/pom.xml"]);

        host.answer_next_prompt(Some(&["clean  install   -DskipTests"]));
        MavenHelper
            .run_command("new-goal", &host.invocation(), Some("/project/pom.xml"))
            .unwrap();
        assert_eq!(
            last_run(&host).1,
            ["-f", "/project/pom.xml", "clean", "install", "-DskipTests"]
        );
        assert_eq!(saved_goals().unwrap(), ["clean install -DskipTests"]);

        // Saved goals lead the picker; its New Goal… saves another.
        host.answer_next_prompt(Some(&[CUSTOM]));
        host.answer_next_prompt(Some(&["verify -Pquality"]));
        MavenHelper
            .run_command("debug-goal", &host.invocation(), None)
            .unwrap();
        assert!(last_run(&host).0.starts_with("Debug mvn verify -Pquality"));
        assert_eq!(
            saved_goals().unwrap(),
            ["verify -Pquality", "clean install -DskipTests"]
        );
        host.answer_next_prompt(Some(&["clean install -DskipTests"]));
        MavenHelper
            .run_command("run-goal", &host.invocation(), None)
            .unwrap();
        assert_eq!(last_run(&host).0, "mvn clean install -DskipTests [app]");

        host.answer_next_prompt(Some(&["verify -Pquality"]));
        MavenHelper
            .run_command("remove-goal", &host.invocation(), None)
            .unwrap();
        assert_eq!(saved_goals().unwrap(), ["clean install -DskipTests"]);
    }

    #[test]
    fn reimport_runs_the_ide_reload() {
        let host = MockHost::install().with_capabilities(CAPS);
        let err = MavenHelper
            .run_command("reimport", &host.invocation(), None)
            .unwrap_err();
        assert!(err.to_string().contains("ide.commands"), "{err}");

        let host = MockHost::install().with_capabilities(&[CAPS, &["ide.commands"]].concat());
        let ran = std::rc::Rc::new(std::cell::Cell::new(false));
        let r = ran.clone();
        host.on_command(MAVEN_RELOAD, move |_| {
            r.set(true);
            Ok(None)
        });
        MavenHelper
            .run_command("reimport", &host.invocation(), None)
            .unwrap();
        assert!(ran.get());
    }

    #[test]
    fn goals_and_extra_args_come_from_settings() {
        let host = MockHost::install().with_capabilities(CAPS);
        assert_eq!(configured_goals().len(), 10);
        host.set_setting("goals", r#""clean,  install\nspring-boot:run""#);
        host.set_setting("extraArgs", r#""-DskipTests -o""#);
        assert_eq!(configured_goals(), ["clean", "install", "spring-boot:run"]);
        assert_eq!(
            maven_args("/p/pom.xml", "install"),
            ["-f", "/p/pom.xml", "-DskipTests", "-o", "install"]
        );
    }
}
