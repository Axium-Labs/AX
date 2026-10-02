//! Composition adapters for the single shared scope registry and manager.
use crate::repl::ReplState;
use anyhow::{Context, Result, bail};
use mcp::McpConfig;
use scoped::{Scope, ScopePolicy, ScopedRegistry};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Kind {
    Skills,
    Mcp,
    Agents,
}
impl Kind {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "skill" | "skills" => Ok(Self::Skills),
            "mcp" => Ok(Self::Mcp),
            "agent" | "agents" => Ok(Self::Agents),
            _ => bail!("Unknown capability kind: {value}"),
        }
    }
    pub const fn key(self) -> &'static str {
        match self {
            Self::Skills => "skills",
            Self::Mcp => "mcp",
            Self::Agents => "agents",
        }
    }
}
pub(crate) fn parse_scope(value: &str) -> Result<Scope> {
    match value {
        "global" => Ok(Scope::Global),
        "project" => Ok(Scope::Project),
        _ => bail!("scope must be global or project"),
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Capability {
    pub description: String,
    pub source: PathBuf,
    pub data: Value,
}
#[derive(Deserialize)]
struct AgentManifest {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default = "yes")]
    enabled: bool,
    /// Instructions are read only when this agent is actually delegated to.
    instructions: PathBuf,
    #[serde(default)]
    tools: Option<Vec<String>>,
}
const fn yes() -> bool {
    true
}
type Definitions = BTreeMap<String, (Capability, bool)>;

fn skill_definitions(roots: &[PathBuf]) -> Result<Definitions> {
    let catalog = skill::SkillCatalog::index_sources(roots)?;
    for issue in catalog.issues() {
        eprintln!("Skill discovery: {issue}");
    }
    Ok(catalog
        .statuses(
            crate::runtime::tools(&[])
                .names()
                .into_iter()
                .chain(["mcp", "memory"]),
        )
        .into_iter()
        .map(|s| {
            let name = s.metadata.name;
            let source = catalog
                .directory(&name)
                .expect("indexed skill")
                .to_path_buf();
            let enabled = s
                .metadata
                .extensions
                .get("enabled")
                .and_then(serde_yaml_bool)
                .unwrap_or(true);
            (
                name,
                (
                    Capability {
                        description: s.metadata.description,
                        source,
                        data: json!({"missing_tools":s.missing_tools}),
                    },
                    enabled,
                ),
            )
        })
        .collect())
}
// Avoid coupling shared scope policy to the Skill frontmatter parser.
fn serde_yaml_bool(value: &impl serde::Serialize) -> Option<bool> {
    serde_json::to_value(value).ok()?.as_bool()
}

fn definitions(
    kind: Kind,
    root: &Path,
    skill_roots: &[PathBuf],
    mcp_path: &Path,
) -> Result<Definitions> {
    match kind {
        Kind::Skills => skill_definitions(skill_roots),
        Kind::Mcp => Ok(McpConfig::load(mcp_path)?
            .servers
            .into_iter()
            .map(|(name, server)| {
                let enabled = server.enabled;
                let description = server.description.clone();
                (
                    name,
                    (
                        Capability {
                            description,
                            source: mcp_path.into(),
                            data: serde_json::to_value(server).expect("serializable MCP config"),
                        },
                        enabled,
                    ),
                )
            })
            .collect()),
        Kind::Agents => {
            let directory = root.join("agents");
            if !directory.exists() {
                return Ok(BTreeMap::new());
            }
            let mut paths = fs::read_dir(&directory)?
                .map(|entry| entry.map(|e| e.path()))
                .collect::<std::io::Result<Vec<_>>>()?;
            paths.sort();
            let mut entries = BTreeMap::new();
            for path in paths
                .into_iter()
                .filter(|p| p.extension().is_some_and(|e| e == "toml"))
            {
                let manifest: AgentManifest = toml::from_str(&fs::read_to_string(&path)?)
                    .with_context(|| format!("Invalid agent {}", path.display()))?;
                validate_name(&manifest.name)?;
                if manifest.instructions.is_absolute()
                    || manifest
                        .instructions
                        .components()
                        .any(|c| matches!(c, std::path::Component::ParentDir))
                {
                    bail!("Agent instructions must be relative to the agents directory");
                }
                let data = json!({"instructions":directory.join(manifest.instructions),"tools":manifest.tools});
                if entries
                    .insert(
                        manifest.name.clone(),
                        (
                            Capability {
                                description: manifest.description,
                                source: path,
                                data,
                            },
                            manifest.enabled,
                        ),
                    )
                    .is_some()
                {
                    bail!("Duplicate agent: {}", manifest.name);
                }
            }
            Ok(entries)
        }
    }
}
fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
        || name == "."
        || name == ".."
    {
        bail!("Invalid capability name: {name}");
    }
    Ok(())
}

fn remove_definition(
    kind: Kind,
    root: &Path,
    name: &str,
    entry: &Capability,
    skill_roots: &[PathBuf],
) -> Result<()> {
    match kind {
        Kind::Skills => {
            let path = fs::canonicalize(&entry.source)?;
            let allowed = skill_roots
                .iter()
                .filter_map(|root| fs::canonicalize(root).ok())
                .any(|root| path.starts_with(&root) && path != root);
            if !allowed {
                bail!(
                    "Cannot remove a Skill outside the selected source roots: {}",
                    path.display()
                );
            }
            fs::remove_dir_all(path)?;
        }
        Kind::Agents => {
            fs::remove_file(&entry.source)?;
            let instructions: PathBuf = serde_json::from_value(entry.data["instructions"].clone())?;
            let remaining = definitions(kind, root, &[], &root.join("mcp.toml"))?;
            if !remaining
                .values()
                .any(|(value, _)| value.data["instructions"] == entry.data["instructions"])
                && instructions.is_file()
            {
                fs::remove_file(instructions)?;
            }
        }
        Kind::Mcp => {
            let mut document = scoped::read_document(&entry.source)?;
            document
                .get_mut("servers")
                .and_then(toml::Value::as_table_mut)
                .context("Missing servers table")?
                .remove(name);
            scoped::write_document(&entry.source, &document)?;
        }
    }
    Ok(())
}
fn add_definition(
    kind: Kind,
    root: &Path,
    mcp_path: &Path,
    scope: Scope,
    name: &str,
    source: &Path,
) -> Result<()> {
    match kind {
        Kind::Skills => {
            let catalog = skill::SkillCatalog::index(
                source.parent().context("Skill directory has no parent")?,
            )?;
            if catalog.directory(name) != Some(source) {
                bail!("Skill source name does not match {name}");
            }
            skill::install_skill_directory(source, &root.join("skills"))?;
        }
        Kind::Mcp => {
            let imported = McpConfig::load(source)?;
            let server = imported
                .servers
                .get(name)
                .context("Source does not contain the requested server")?;
            let path = if scope == Scope::Project {
                mcp_path.to_path_buf()
            } else {
                root.join("mcp.toml")
            };
            let mut document = scoped::read_document(&path)?;
            let servers = document
                .as_table_mut()
                .context("Expected table")?
                .entry("servers".to_owned())
                .or_insert_with(|| toml::Value::Table(toml::map::Map::new()))
                .as_table_mut()
                .context("Expected servers table")?;
            if servers.contains_key(name) {
                bail!("Server already exists: {name}");
            }
            servers.insert(name.into(), toml::Value::try_from(server)?);
            scoped::write_document(&path, &document)?;
        }
        Kind::Agents => {
            let manifest: AgentManifest = toml::from_str(&fs::read_to_string(source)?)?;
            validate_name(&manifest.name)?;
            if manifest.name != name {
                bail!("Agent source name does not match {name}");
            }
            if manifest.instructions.is_absolute()
                || manifest
                    .instructions
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                bail!("Agent instructions must be relative");
            }
            let directory = root.join("agents");
            let target = directory.join(format!("{name}.toml"));
            if target.exists() {
                bail!("Agent already exists: {name}");
            }
            let body = fs::read(
                source
                    .parent()
                    .context("Missing parent")?
                    .join(&manifest.instructions),
            )?;
            fs::create_dir_all(&directory)?;
            let body_target = directory.join(&manifest.instructions);
            if body_target.exists() {
                bail!("Instructions already exist");
            }
            if let Some(parent) = body_target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(body_target, body)?;
            fs::copy(source, target)?;
        }
    }
    Ok(())
}

impl ReplState {
    pub(crate) fn configure_scoped_subagents(&mut self) -> Result<()> {
        let state = self;
        let mut subagent_config = crate::config::AxConfig::load()?.subagent;
        if let Some(project) =
            scoped::read_document(&state.project_root.join(".ax/config.toml"))?.get("subagent")
        {
            let mut effective = toml::Value::try_from(subagent_config)?;
            if let (Some(effective), Some(project)) = (effective.as_table_mut(), project.as_table())
            {
                effective.extend(project.clone());
            }
            subagent_config = effective.try_into()?;
        }
        let templates = if subagent_config.enabled && subagent_config.max_depth > 0 {
            state
                .capability_registry(Kind::Agents)?
                .effective()
                .map(|entry| {
                    Ok(runtime_core::AgentTemplate {
                        name: entry.name.clone(),
                        description: entry.value.description.clone(),
                        instructions: serde_json::from_value(
                            entry.value.data["instructions"].clone(),
                        )?,
                        tools: serde_json::from_value(entry.value.data["tools"].clone())?,
                    })
                })
                .collect::<Result<Vec<_>>>()?
        } else {
            Vec::new()
        };
        let runtime = state.runtime.as_mut().expect("runtime initialized");
        runtime.configure_subagents(subagent_config);
        runtime.configure_agent_templates(templates);
        Ok(())
    }
    pub(crate) fn capability_root(&self, scope: Scope) -> PathBuf {
        match scope {
            Scope::Global => self.capability_home.clone(),
            Scope::Project => self.project_root.join(".ax"),
        }
    }
    pub(crate) fn capability_registry(&self, kind: Kind) -> Result<ScopedRegistry<Capability>> {
        if let Some(registry) = self.capability_registries.borrow().get(&kind) {
            return Ok(registry.clone());
        }
        let global = self.capability_root(Scope::Global);
        let project = self.capability_root(Scope::Project);
        let global_policy = scoped::policy(&global.join("config.toml"), kind.key())?;
        let mut project_policy = scoped::policy(&project.join("config.toml"), kind.key())?;
        // Read old explicit skill choices until that skill is changed through the manager.
        if kind == Kind::Skills
            && let Ok(bytes) = fs::read(self.data_dir.join("disabled-skills.json"))
        {
            for name in serde_json::from_slice::<Vec<String>>(&bytes)? {
                project_policy.overrides.entry(name).or_insert(false);
            }
        }
        let global_defs = definitions(
            kind,
            &global,
            &[global.join("skills")],
            &global.join("mcp.toml"),
        )?;
        let project_defs = definitions(
            kind,
            &project,
            &[
                project.join("skills"),
                self.skills_dir.clone(),
                self.evolution_root().join("live"),
            ],
            &self.mcp_config,
        )?;
        let registry = ScopedRegistry::build(
            global_defs.into_iter().map(|(n, (v, e))| (n, v, e)),
            project_defs.into_iter().map(|(n, (v, e))| (n, v, e)),
            &global_policy,
            &project_policy,
        );
        self.capability_registries
            .borrow_mut()
            .insert(kind, registry.clone());
        Ok(registry)
    }
    pub(crate) fn capability_rows(&self, kind: Kind, scope: Option<Scope>) -> Result<Vec<Value>> {
        // Global configuration shows its own definitions even when the project shadows them.
        let registry = if scope == Some(Scope::Global) {
            let root = self.capability_root(Scope::Global);
            let definitions =
                definitions(kind, &root, &[root.join("skills")], &root.join("mcp.toml"))?;
            ScopedRegistry::build(
                definitions.into_iter().map(|(n, (v, e))| (n, v, e)),
                [],
                &scoped::policy(&root.join("config.toml"), kind.key())?,
                &ScopePolicy::default(),
            )
        } else {
            self.capability_registry(kind)?
        };
        Ok(registry.entries().map(|entry| json!({"name":entry.name,"scope":entry.scope,"enabled":entry.enabled,"status":entry.status(),"description":entry.value.description,"source":entry.value.source,"missing_tools":entry.value.data.get("missing_tools").unwrap_or(&json!([])),"capabilities":entry.value.data.get("capabilities").unwrap_or(&json!([]))})).collect())
    }
    pub(crate) fn effective_mcp_config(&self) -> Result<McpConfig> {
        let registry = self.capability_registry(Kind::Mcp)?;
        Ok(McpConfig {
            servers: registry
                .effective()
                .map(|entry| {
                    let mut server: mcp::ServerConfig =
                        serde_json::from_value(entry.value.data.clone())?;
                    server.enabled = entry.enabled;
                    Ok((entry.name.clone(), server))
                })
                .collect::<Result<_>>()?,
        })
    }
    pub(crate) fn manage_capability(
        &mut self,
        kind: Kind,
        scope: Scope,
        action: &str,
        name: &str,
        source: Option<&Path>,
    ) -> Result<()> {
        validate_name(name)?;
        let root = self.capability_root(scope);
        fs::create_dir_all(&root)?;
        let lease = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join(".capabilities.lock"))?;
        fs2::FileExt::lock_exclusive(&lease)?;
        // Read current metadata/policy under the same mutation lease.
        self.capability_registries.borrow_mut().clear();
        let config = root.join("config.toml");
        let mut policy = scoped::policy(&config, kind.key())?;
        let registry = if scope == Scope::Global {
            let defs = definitions(kind, &root, &[root.join("skills")], &root.join("mcp.toml"))?;
            ScopedRegistry::build(
                defs.into_iter().map(|(n, (v, e))| (n, v, e)),
                [],
                &policy,
                &ScopePolicy::default(),
            )
        } else {
            self.capability_registry(kind)?
        };
        if scope == Scope::Project {
            crate::project_identity::ensure_portable(&self.project_root, &self.project_id)?;
        }
        match action {
            "enable" | "disable" => {
                let entry = registry
                    .get(name)
                    .with_context(|| format!("Unknown {}: {name}", kind.key()))?;
                let enabled = action == "enable";
                if scope == Scope::Project && entry.scope == Scope::Global {
                    if enabled {
                        policy.disabled_global.remove(name);
                    } else {
                        policy.disabled_global.insert(name.into());
                    }
                }
                policy.overrides.insert(name.into(), enabled);
            }
            "remove" => {
                let entry = registry
                    .get(name)
                    .with_context(|| format!("Unknown {}: {name}", kind.key()))?;
                if entry.scope == scope {
                    let mut skill_roots = vec![root.join("skills")];
                    if scope == Scope::Project {
                        skill_roots.push(self.skills_dir.clone());
                    }
                    remove_definition(kind, &root, name, &entry.value, &skill_roots)?;
                    policy.overrides.remove(name);
                } else {
                    policy.disabled_global.insert(name.into());
                }
            }
            "add" => {
                let source = fs::canonicalize(source.context("add requires a source path")?)?;
                add_definition(kind, &root, &self.mcp_config, scope, name, &source)?;
                policy.overrides.insert(name.into(), true);
                policy.disabled_global.remove(name);
            }
            _ => bail!("action must be list, enable, disable, add or remove"),
        }
        scoped::save_policy(&config, kind.key(), &policy)?;
        if action == "enable" || action == "add" {
            // A top-level mask is shorthand for all capability kinds.
            let mut document = scoped::read_document(&config)?;
            if let Some(disabled) = document
                .get_mut("disabled_global")
                .and_then(toml::Value::as_array_mut)
            {
                disabled.retain(|value| value.as_str() != Some(name));
                scoped::write_document(&config, &document)?;
            }
        }
        self.invalidate_runtime();
        self.skill_catalog = None;
        self.capability_registries.borrow_mut().clear();
        self.mcp_manager = None;
        self.mcp_tools.clear();
        // Raw history stays intact; stale active Skill context is removed before reuse.
        self.loaded_messages
            .retain(|message| crate::repl::active_skill_name(message).is_none());
        self.active_skills.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (PathBuf, ReplState) {
        let root = std::env::temp_dir().join(format!("ax-scoped-{}", uuid::Uuid::new_v4()));
        let project = root.join("project");
        fs::create_dir_all(&project).unwrap();
        let mut state =
            ReplState::new_in_project(root.join("state"), project.join("skills"), None, &project)
                .unwrap();
        state.capability_home = root.join("global");
        (root, state)
    }
    fn install(root: &Path, name: &str, enabled: bool) {
        let skills = root.join("skills").join(name);
        fs::create_dir_all(&skills).unwrap();
        fs::write(skills.join("SKILL.md"), format!("---\nname: {name}\ndescription: Review code changes\nenabled: {enabled}\n---\nInstructions never needed during listing\n")).unwrap();
        let agents = root.join("agents");
        fs::create_dir_all(&agents).unwrap();
        fs::write(agents.join(format!("{name}.toml")), format!("name = \"{name}\"\ndescription = \"Review changes\"\nenabled = {enabled}\ninstructions = \"missing-{name}.md\"\n")).unwrap();
        let mut document = scoped::read_document(&root.join("mcp.toml")).unwrap();
        let servers = document
            .as_table_mut()
            .unwrap()
            .entry("servers")
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()))
            .as_table_mut()
            .unwrap();
        servers.insert(
            name.into(),
            toml::from_str::<toml::Value>(&format!(
                "transport = \"stdio\"\ncommand = \"must-never-start\"\nenabled = {enabled}\n"
            ))
            .unwrap(),
        );
        scoped::write_document(&root.join("mcp.toml"), &document).unwrap();
    }
    #[test]
    fn all_three_adapters_share_override_mask_status_and_lazy_effective_sets() {
        let (root, mut state) = fixture();
        let global = state.capability_root(Scope::Global);
        let project = state.capability_root(Scope::Project);
        install(&global, "reviewer", true);
        install(&global, "browser", true);
        install(&global, "off", false);
        install(&project, "reviewer", true);
        for kind in [Kind::Skills, Kind::Mcp, Kind::Agents] {
            let registry = state.capability_registry(kind).unwrap();
            assert_eq!(registry.get("reviewer").unwrap().scope, Scope::Project);
            assert_eq!(registry.effective().count(), 2);
            state
                .manage_capability(kind, Scope::Project, "disable", "browser", None)
                .unwrap();
            let registry = state.capability_registry(kind).unwrap();
            assert_eq!(registry.get("browser").unwrap().status(), "disabled here");
            assert_eq!(registry.effective().count(), 1);
            let global_rows = state.capability_rows(kind, Some(Scope::Global)).unwrap();
            assert!(
                global_rows
                    .iter()
                    .any(|row| row["name"] == "browser" && row["enabled"] == true)
            );
            state
                .manage_capability(kind, Scope::Project, "enable", "browser", None)
                .unwrap();
            assert_eq!(
                state.capability_registry(kind).unwrap().effective().count(),
                2
            );
            state
                .manage_capability(kind, Scope::Global, "disable", "browser", None)
                .unwrap();
            // The explicit project enable remains an override of the global setting.
            assert!(
                state
                    .capability_registry(kind)
                    .unwrap()
                    .get("browser")
                    .unwrap()
                    .enabled
            );
        }
        let config = state.effective_mcp_config().unwrap();
        assert!(!config.servers.contains_key("off"));
        state
            .manage_capability(Kind::Mcp, Scope::Project, "enable", "off", None)
            .unwrap();
        assert!(state.effective_mcp_config().unwrap().servers["off"].enabled);
        assert_eq!(mcp::McpManager::new(config).connected_server_count(), 0);
        // Agent manifests refer to absent bodies: listing and resolution still succeeded.
        assert!(
            state
                .capability_registry(Kind::Agents)
                .unwrap()
                .get("off")
                .is_some()
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn project_masks_do_not_leak_and_survive_project_moves() {
        let (root, mut state) = fixture();
        install(&state.capability_root(Scope::Global), "reviewer", true);
        let id = state.project_id.clone();
        state
            .manage_capability(Kind::Agents, Scope::Project, "disable", "reviewer", None)
            .unwrap();
        let other = root.join("other");
        fs::create_dir_all(&other).unwrap();
        let mut second =
            ReplState::new_in_project(root.join("state-two"), other.join("skills"), None, &other)
                .unwrap();
        second.capability_home = state.capability_home.clone();
        assert!(
            second
                .capability_registry(Kind::Agents)
                .unwrap()
                .get("reviewer")
                .unwrap()
                .enabled
        );
        let moved = root.join("moved");
        fs::rename(&state.project_root, &moved).unwrap();
        let mut moved_state =
            ReplState::new_in_project(root.join("state"), moved.join("skills"), None, &moved)
                .unwrap();
        moved_state.capability_home = state.capability_home.clone();
        assert_eq!(moved_state.project_id, id);
        assert_eq!(
            moved_state
                .capability_registry(Kind::Agents)
                .unwrap()
                .get("reviewer")
                .unwrap()
                .status(),
            "disabled here"
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn remove_project_definition_reveals_global_and_keeps_unrelated_config() {
        let (root, mut state) = fixture();
        install(&state.capability_root(Scope::Global), "reviewer", true);
        install(&state.capability_root(Scope::Project), "reviewer", false);
        let config = state.project_root.join(".ax/config.toml");
        fs::write(&config, "[subagent]\nenabled = true\n").unwrap();
        for kind in [Kind::Skills, Kind::Mcp, Kind::Agents] {
            state
                .manage_capability(kind, Scope::Project, "remove", "reviewer", None)
                .unwrap();
            assert_eq!(
                state
                    .capability_registry(kind)
                    .unwrap()
                    .get("reviewer")
                    .unwrap()
                    .scope,
                Scope::Global
            );
        }
        assert_eq!(
            scoped::read_document(&config).unwrap()["subagent"]["enabled"].as_bool(),
            Some(true)
        );
        fs::remove_dir_all(root).unwrap();
    }
}
