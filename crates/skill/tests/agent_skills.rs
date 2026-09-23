use skill::{
    ResourceKind, SkillCatalog, create_skill_directory, install_skill_directory,
    validate_skill_directory,
};
use std::{
    fs,
    path::{Path, PathBuf},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("ax-skill-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn skill(&self, name: &str, body: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("SKILL.md"), body).unwrap();
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn parses_standard_fields_and_ignores_valid_extensions() {
    let fixture = Fixture::new();
    let directory = fixture.skill("code-review", "---\nname: code-review\ndescription: Review diffs and commits for regressions when inspecting code.\nlicense: Apache-2.0\ncompatibility: Requires git.\nmetadata:\n  author: axium-labs\n  version: '1.0'\nallowed-tools: filesystem search shell\nother: value\n---\n# Review\nInspect the diff.\n");
    let catalog = SkillCatalog::index(&fixture.0).unwrap();
    assert_eq!(catalog.len(), 1);
    assert!(catalog.issues().is_empty());
    let metadata = &catalog.statuses(["filesystem"])[0].metadata;
    assert_eq!(metadata.license.as_deref(), Some("Apache-2.0"));
    assert_eq!(metadata.compatibility.as_deref(), Some("Requires git."));
    assert_eq!(metadata.metadata["version"], "1.0");
    assert_eq!(
        metadata.allowed_tools.as_deref(),
        Some("filesystem search shell")
    );
    assert!(metadata.extensions.contains_key("other"));
    assert_eq!(
        catalog.load("code-review").unwrap().instructions,
        "# Review\nInspect the diff."
    );
    validate_skill_directory(&directory).unwrap();
}

#[test]
fn discovery_reads_only_frontmatter_then_loads_body_and_resources() {
    let fixture = Fixture::new();
    let directory = fixture.skill(
        "inspect",
        "---\nname: inspect\ndescription: Inspect files when reviewing source code.\n---\n",
    );
    let mut bytes = fs::read(directory.join("SKILL.md")).unwrap();
    bytes.extend_from_slice(&[0xff, 0xfe]);
    fs::write(directory.join("SKILL.md"), bytes).unwrap();
    let catalog = SkillCatalog::index(&fixture.0).unwrap();
    assert_eq!(catalog.len(), 1);
    assert!(catalog.load("inspect").is_err());
    fs::write(directory.join("SKILL.md"), "---\nname: inspect\ndescription: Inspect files when reviewing source code.\n---\nRead references/guide.md.\n").unwrap();
    for kind in ["scripts", "references", "assets"] {
        fs::create_dir(directory.join(kind)).unwrap();
        fs::write(directory.join(kind).join("item"), "resource").unwrap();
    }
    assert_eq!(
        catalog.load("inspect").unwrap().instructions,
        "Read references/guide.md."
    );
    for kind in [
        ResourceKind::Scripts,
        ResourceKind::References,
        ResourceKind::Assets,
    ] {
        assert_eq!(catalog.resources("inspect", kind).unwrap().len(), 1);
    }
}

#[test]
fn description_routes_without_private_triggers() {
    let fixture = Fixture::new();
    fixture.skill("code-review", "---\nname: code-review\ndescription: Review diffs and commits for regressions when inspecting modified code.\n---\nReview carefully.");
    fixture.skill("skill-installer", "---\nname: skill-installer\ndescription: Install Agent Skills when importing a skill directory.\n---\nValidate first.");
    let catalog = SkillCatalog::index(&fixture.0).unwrap();
    assert_eq!(
        catalog
            .route("Inspect modified code for regressions", [])
            .unwrap()
            .name,
        "code-review"
    );
    assert_eq!(
        catalog.route("Import a skill directory", []).unwrap().name,
        "skill-installer"
    );
    assert!(catalog.route("What time is it?", []).is_none());
}

#[test]
fn malformed_packages_are_isolated_and_duplicates_are_stable() {
    let project = Fixture::new();
    let global = Fixture::new();
    project.skill(
        "good",
        "---\nname: good\ndescription: Review files when requested.\n---\nProject body",
    );
    project.skill("bad", "---\nname: bad\ndescription: [broken\n---\nBody");
    project.skill(
        "bad--name",
        "---\nname: bad--name\ndescription: A valid description.\n---\nBody",
    );
    project.skill(
        "wrong-folder",
        "---\nname: other\ndescription: A valid description.\n---\nBody",
    );
    project.skill("no-frontmatter", "# Instructions only");
    global.skill(
        "good",
        "---\nname: good\ndescription: Review files when requested.\n---\nGlobal body",
    );
    let catalog = SkillCatalog::index_sources([&project.0, &global.0]).unwrap();
    assert_eq!(catalog.len(), 1);
    assert_eq!(catalog.issues().len(), 5);
    assert_eq!(catalog.load("good").unwrap().instructions, "Project body");
    let repeated_root = SkillCatalog::index_sources([&global.0, &global.0]).unwrap();
    assert_eq!(repeated_root.len(), 1);
    assert!(repeated_root.issues().is_empty());
}

#[test]
fn standard_wins_over_legacy_and_legacy_remains_usable() {
    let fixture = Fixture::new();
    let modern = fixture.skill(
        "modern",
        "---\nname: modern\ndescription: Review modern code changes.\n---\nStandard body",
    );
    fs::write(
        modern.join("skill.toml"),
        "name = 'obsolete'\ndescription = 'Old'\n",
    )
    .unwrap();
    fs::write(modern.join("instructions.md"), "Legacy body").unwrap();
    let old = fixture.0.join("old-directory");
    fs::create_dir(&old).unwrap();
    fs::write(
        old.join("skill.toml"),
        "name = 'old'\ndescription = 'Review old code'\nrequired_tools = ['shell']\n",
    )
    .unwrap();
    fs::write(old.join("instructions.md"), "Old body").unwrap();
    let catalog = SkillCatalog::index(&fixture.0).unwrap();
    assert_eq!(catalog.len(), 2);
    assert_eq!(
        catalog.load("modern").unwrap().instructions,
        "Standard body"
    );
    assert_eq!(catalog.load("old").unwrap().instructions, "Old body");
    assert!(
        !catalog
            .statuses(["filesystem"])
            .iter()
            .find(|s| s.metadata.name == "old")
            .unwrap()
            .available()
    );
}

#[test]
fn installer_copies_standard_and_migrates_legacy() {
    let source = Fixture::new();
    let target = Fixture::new();
    let standard = source.skill("inspect", "---\nname: inspect\ndescription: Inspect code when reviewing files.\n---\nUse references/guide.md.\n");
    fs::create_dir(standard.join("references")).unwrap();
    fs::write(standard.join("references/guide.md"), "Guide").unwrap();
    let installed = install_skill_directory(&standard, &target.0).unwrap();
    assert_eq!(
        fs::read_to_string(installed.join("SKILL.md")).unwrap(),
        fs::read_to_string(standard.join("SKILL.md")).unwrap()
    );
    assert!(installed.join("references/guide.md").exists());
    assert!(install_skill_directory(&standard, &target.0).is_err());
    let legacy = source.0.join("legacy-source");
    fs::create_dir(&legacy).unwrap();
    fs::write(legacy.join("skill.toml"), "name = 'legacy'\ndescription = 'Review old code when requested.'\nrequired_tools = ['shell']\n").unwrap();
    fs::write(legacy.join("instructions.md"), "Old instructions.").unwrap();
    let migrated = install_skill_directory(&legacy, &target.0).unwrap();
    assert!(migrated.join("SKILL.md").exists());
    assert!(!migrated.join("skill.toml").exists());
    assert_eq!(
        validate_skill_directory(&migrated)
            .unwrap()
            .metadata
            .metadata["ax.required-tools"],
        "shell"
    );
}

#[test]
fn creator_emits_valid_standard_package_and_rejects_invalid_names() {
    let fixture = Fixture::new();
    let directory = create_skill_directory(
        &fixture.0,
        "release-notes",
        "Write release notes when preparing a software release.",
        "# Release notes\nSummarize user-visible changes.",
    )
    .unwrap();
    assert!(directory.join("SKILL.md").exists());
    assert!(!directory.join("skill.toml").exists());
    assert_eq!(
        validate_skill_directory(&directory).unwrap().metadata.name,
        "release-notes"
    );
    assert!(create_skill_directory(&fixture.0, "release-notes", "Valid", "Body").is_err());
    for name in ["BadName", "double--hyphen", "-leading", "trailing-"] {
        assert!(create_skill_directory(&fixture.0, name, "Valid", "Body").is_err());
    }
    assert!(create_skill_directory(&fixture.0, "empty", " ", "Body").is_err());
    assert!(
        create_skill_directory(&fixture.0, "long-description", &"x".repeat(1025), "Body").is_err()
    );
    assert!(create_skill_directory(&fixture.0, &"a".repeat(65), "Valid", "Body").is_err());
}

#[test]
fn bundled_creator_and_installer_describe_standard_output() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills");
    let catalog = SkillCatalog::index(root).unwrap();
    assert_eq!(catalog.len(), 4);
    assert!(catalog.issues().is_empty());
    for name in ["coding", "code-review", "skill-creator", "skill-installer"] {
        validate_skill_directory(catalog.directory(name).unwrap()).unwrap();
    }
    let creator = catalog.load("skill-creator").unwrap().instructions;
    let installer = catalog.load("skill-installer").unwrap().instructions;
    assert!(creator.contains("SKILL.md") && creator.contains("scripts/"));
    assert!(installer.contains("validate") && installer.contains("SKILL.md"));
}
