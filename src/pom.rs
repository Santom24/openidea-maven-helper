//! Just enough of `pom.xml` for Maven Helper: the module's coordinates, where
//! a direct dependency is declared, and the text edit that adds an
//! `<exclusion>` to it. A small tag scanner, not a full XML parser: comments,
//! processing instructions and CDATA are skipped, and only element structure
//! and text content are read.

/// One element: its slash-joined path from the root (`project/dependencies/dependency`)
/// and byte offsets into the text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Element {
    pub path: String,
    /// Offset of `<`.
    pub start: usize,
    /// Offset just after the opening tag's `>`.
    pub content_start: usize,
    /// Offset of the closing tag's `<` (equal to `content_start` for `<a/>`).
    pub content_end: usize,
    /// Offset just after the closing tag's `>`.
    pub end: usize,
}

impl Element {
    pub fn text<'a>(&self, src: &'a str) -> &'a str {
        src[self.content_start..self.content_end].trim()
    }

    fn contains(&self, other: &Element) -> bool {
        other.start > self.start && other.end <= self.end
    }
}

/// Every element of `src`, in document order. Malformed input yields what
/// could be matched; it never panics.
pub fn elements(src: &str) -> Vec<Element> {
    let bytes = src.as_bytes();
    let mut out: Vec<Element> = Vec::new();
    // (name, index in `out`)
    let mut stack: Vec<(String, usize)> = Vec::new();
    let mut i = 0;
    while let Some(rel) = src[i..].find('<') {
        let lt = i + rel;
        let rest = &src[lt..];
        if rest.starts_with("<!--") {
            i = rest.find("-->").map_or(src.len(), |e| lt + e + 3);
            continue;
        }
        if rest.starts_with("<![CDATA[") {
            i = rest.find("]]>").map_or(src.len(), |e| lt + e + 3);
            continue;
        }
        if rest.starts_with("<?") || rest.starts_with("<!") {
            i = rest.find('>').map_or(src.len(), |e| lt + e + 1);
            continue;
        }
        let Some(gt) = tag_end(bytes, lt) else { break };
        let inner = &src[lt + 1..gt];
        if let Some(name) = inner.strip_prefix('/') {
            let name = name.trim();
            if let Some(pos) = stack.iter().rposition(|(n, _)| n == name) {
                let (_, idx) = stack[pos].clone();
                stack.truncate(pos);
                out[idx].content_end = lt;
                out[idx].end = gt + 1;
            }
        } else {
            let self_closing = inner.ends_with('/');
            let name: String = inner
                .trim_end_matches('/')
                .split(|c: char| c.is_whitespace())
                .next()
                .unwrap_or("")
                .to_owned();
            if !name.is_empty() {
                let path = match stack.last() {
                    Some((_, parent)) => format!("{}/{}", out[*parent].path, name),
                    None => name.clone(),
                };
                out.push(Element {
                    path,
                    start: lt,
                    content_start: gt + 1,
                    content_end: gt + 1,
                    end: gt + 1,
                });
                if !self_closing {
                    stack.push((name, out.len() - 1));
                }
            }
        }
        i = gt + 1;
    }
    out
}

/// The `>` closing the tag that starts at `lt`, skipping quoted attribute values.
fn tag_end(bytes: &[u8], lt: usize) -> Option<usize> {
    let mut quote: Option<u8> = None;
    for (j, &b) in bytes.iter().enumerate().skip(lt + 1) {
        match quote {
            Some(q) if b == q => quote = None,
            Some(_) => {}
            None if b == b'"' || b == b'\'' => quote = Some(b),
            None if b == b'>' => return Some(j),
            None => {}
        }
    }
    None
}

fn child<'e>(all: &'e [Element], parent: &Element, name: &str) -> Option<&'e Element> {
    let path = format!("{}/{}", parent.path, name);
    all.iter().find(|e| e.path == path && parent.contains(e))
}

/// `groupId:artifactId:version` of the module a POM describes; the group and
/// version fall back to the `<parent>`'s.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Coordinates {
    pub group: String,
    pub artifact: String,
    pub version: String,
}

impl Coordinates {
    pub fn key(&self) -> String {
        format!("{}:{}", self.group, self.artifact)
    }
}

pub fn coordinates(src: &str) -> Option<Coordinates> {
    let all = elements(src);
    let text = |path: &str| {
        all.iter()
            .find(|e| e.path == path)
            .map(|e| e.text(src).to_owned())
    };
    let artifact = text("project/artifactId")?;
    let group = text("project/groupId")
        .or_else(|| text("project/parent/groupId"))
        .unwrap_or_default();
    let version = text("project/version")
        .or_else(|| text("project/parent/version"))
        .unwrap_or_default();
    Some(Coordinates {
        group,
        artifact,
        version,
    })
}

/// The `<dependency>` element of `project/dependencies` declaring `group:artifact`.
/// Dependencies under `<dependencyManagement>` or a `<profile>` are not
/// candidates: they are not what brings an artifact into the build here.
pub fn find_dependency(src: &str, group: &str, artifact: &str) -> Option<Element> {
    let all = elements(src);
    all.iter()
        .filter(|e| e.path == "project/dependencies/dependency")
        .find(|dep| {
            let g = child(&all, dep, "groupId").map(|e| e.text(src));
            let a = child(&all, dep, "artifactId").map(|e| e.text(src));
            g == Some(group) && a == Some(artifact)
        })
        .cloned()
}

/// The indentation unit the file uses: that of `<project>`'s first child, else 4 spaces.
fn indent_unit(src: &str, all: &[Element]) -> String {
    all.iter()
        .find(|e| e.path.matches('/').count() == 1)
        .map(|e| line_indent(src, e.start))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "    ".into())
}

/// The whitespace between the start of `at`'s line and `at`, if that is all there is.
fn line_indent(src: &str, at: usize) -> String {
    let line_start = src[..at].rfind('\n').map_or(0, |n| n + 1);
    let prefix = &src[line_start..at];
    if prefix.chars().all(char::is_whitespace) {
        prefix.to_owned()
    } else {
        String::new()
    }
}

/// An insertion of `text` at byte offset `at`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Insert {
    pub at: usize,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExcludeError {
    /// `group:artifact` is not declared in this POM's `<dependencies>` (it may
    /// come from a parent POM or a profile).
    NotDeclared(String),
    /// The exclusion is already there.
    AlreadyExcluded,
}

/// The insertion that adds `<exclusion>` of `ex_group:ex_artifact` to the
/// direct dependency `group:artifact`, in the file's indentation.
pub fn exclusion_edit(
    src: &str,
    group: &str,
    artifact: &str,
    ex_group: &str,
    ex_artifact: &str,
) -> Result<Insert, ExcludeError> {
    let all = elements(src);
    let dep = find_dependency(src, group, artifact)
        .ok_or_else(|| ExcludeError::NotDeclared(format!("{group}:{artifact}")))?;
    let unit = indent_unit(src, &all);
    let nl = if src.contains("\r\n") { "\r\n" } else { "\n" };

    if let Some(exclusions) = child(&all, &dep, "exclusions") {
        let already = all
            .iter()
            .filter(|e| {
                e.path == format!("{}/exclusion", exclusions.path) && exclusions.contains(e)
            })
            .any(|ex| {
                child(&all, ex, "groupId").map(|e| e.text(src)) == Some(ex_group)
                    && child(&all, ex, "artifactId").map(|e| e.text(src)) == Some(ex_artifact)
            });
        if already {
            return Err(ExcludeError::AlreadyExcluded);
        }
        if exclusions.content_end > exclusions.content_start {
            let ind = line_indent(src, exclusions.start);
            let close_ind = line_indent(src, exclusions.content_end);
            let p = if close_ind.is_empty() {
                format!("{nl}{ind}{unit}")
            } else {
                unit.clone()
            };
            let i1 = format!("{ind}{unit}");
            let i2 = format!("{i1}{unit}");
            let text = format!(
                "{p}<exclusion>{nl}{i2}<groupId>{ex_group}</groupId>{nl}{i2}<artifactId>{ex_artifact}</artifactId>{nl}{i1}</exclusion>{nl}{close_ind}"
            );
            return Ok(Insert {
                at: exclusions.content_end,
                text,
            });
        }
    }

    // No (usable) <exclusions>: add one before </dependency>.
    if dep.content_end == dep.content_start {
        return Err(ExcludeError::NotDeclared(format!("{group}:{artifact}")));
    }
    let ind = line_indent(src, dep.start);
    let close_ind = line_indent(src, dep.content_end);
    let p = if close_ind.is_empty() {
        format!("{nl}{ind}{unit}")
    } else {
        unit.clone()
    };
    let i1 = format!("{ind}{unit}");
    let i2 = format!("{i1}{unit}");
    let i3 = format!("{i2}{unit}");
    let text = format!(
        "{p}<exclusions>{nl}{i2}<exclusion>{nl}{i3}<groupId>{ex_group}</groupId>{nl}{i3}<artifactId>{ex_artifact}</artifactId>{nl}{i2}</exclusion>{nl}{i1}</exclusions>{nl}{close_ind}"
    );
    Ok(Insert {
        at: dep.content_end,
        text,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const POM: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<project xmlns="http://maven.apache.org/POM/4.0.0">
  <modelVersion>4.0.0</modelVersion>
  <parent>
    <groupId>com.acme</groupId>
    <artifactId>parent</artifactId>
    <version>1.2.0</version>
  </parent>
  <artifactId>app</artifactId>
  <!-- <groupId>not.this</groupId> -->
  <dependencyManagement>
    <dependencies>
      <dependency>
        <groupId>org.slf4j</groupId>
        <artifactId>slf4j-api</artifactId>
        <version>2.0.9</version>
      </dependency>
    </dependencies>
  </dependencyManagement>
  <dependencies>
    <dependency>
      <groupId>org.springframework</groupId>
      <artifactId>spring-core</artifactId>
      <version>6.1.0</version>
    </dependency>
    <dependency>
      <groupId>org.slf4j</groupId>
      <artifactId>slf4j-api</artifactId>
      <exclusions>
        <exclusion>
          <groupId>a</groupId>
          <artifactId>b</artifactId>
        </exclusion>
      </exclusions>
    </dependency>
  </dependencies>
</project>
"#;

    #[test]
    fn reads_coordinates_with_parent_fallback() {
        let c = coordinates(POM).unwrap();
        assert_eq!(c.key(), "com.acme:app");
        assert_eq!(c.version, "1.2.0");
        assert!(coordinates("<project><groupId>x</groupId></project>").is_none());
    }

    #[test]
    fn finds_only_direct_dependencies() {
        let dep = find_dependency(POM, "org.slf4j", "slf4j-api").unwrap();
        assert!(
            POM[dep.start..dep.end].contains("<exclusions>"),
            "not the managed one"
        );
        assert!(find_dependency(POM, "org.springframework", "spring-core").is_some());
        assert!(
            find_dependency(POM, "a", "b").is_none(),
            "exclusions are not dependencies"
        );
    }

    fn apply(src: &str, ins: &Insert) -> String {
        format!("{}{}{}", &src[..ins.at], ins.text, &src[ins.at..])
    }

    #[test]
    fn adds_an_exclusions_block() {
        let ins = exclusion_edit(
            POM,
            "org.springframework",
            "spring-core",
            "commons-logging",
            "commons-logging",
        )
        .unwrap();
        let out = apply(POM, &ins);
        assert!(out.contains(
            "      <version>6.1.0</version>
      <exclusions>
        <exclusion>
          <groupId>commons-logging</groupId>
          <artifactId>commons-logging</artifactId>
        </exclusion>
      </exclusions>
    </dependency>"
        ));
        assert_eq!(
            find_dependency(&out, "org.springframework", "spring-core").map(|_| ()),
            Some(())
        );
    }

    #[test]
    fn appends_to_existing_exclusions() {
        let ins = exclusion_edit(POM, "org.slf4j", "slf4j-api", "c", "d").unwrap();
        let out = apply(POM, &ins);
        assert!(out.contains(
            "          <artifactId>b</artifactId>
        </exclusion>
        <exclusion>
          <groupId>c</groupId>
          <artifactId>d</artifactId>
        </exclusion>
      </exclusions>"
        ));
        assert_eq!(
            exclusion_edit(&out, "org.slf4j", "slf4j-api", "c", "d"),
            Err(ExcludeError::AlreadyExcluded)
        );
        assert_eq!(
            exclusion_edit(POM, "org.slf4j", "slf4j-api", "a", "b"),
            Err(ExcludeError::AlreadyExcluded)
        );
    }

    #[test]
    fn undeclared_dependency_is_an_error() {
        assert_eq!(
            exclusion_edit(POM, "x", "y", "c", "d"),
            Err(ExcludeError::NotDeclared("x:y".into()))
        );
    }

    #[test]
    fn tolerates_garbage() {
        for s in [
            "",
            "<",
            "<project",
            "<!-- x",
            "</a></b>",
            "<a b='>'><c/></a>",
        ] {
            let _ = elements(s);
            let _ = coordinates(s);
        }
        let all = elements("<a b='>'><c/></a>");
        assert_eq!(
            all.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(),
            ["a", "a/c"]
        );
    }
}
