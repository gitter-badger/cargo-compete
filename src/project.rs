use crate::shell::Shell;
use anyhow::{bail, Context as _};
use cargo_metadata::{Metadata, MetadataCommand, Package, Resolve, Target};
use derivative::Derivative;
use easy_ext::ext;
use heck::KebabCase as _;
use indexmap::IndexMap;
use itertools::Itertools as _;
use liquid::object;
use serde::{de::Error as _, Deserialize, Deserializer};
use snowchains_core::web::PlatformKind;
use std::{
    collections::BTreeMap,
    env,
    path::{Path, PathBuf},
    str,
};
use url::Url;

#[derive(Deserialize, Derivative)]
#[derivative(Debug)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct CargoCompeteConfig {
    pub(crate) new_workspace_member: NewWorkspaceMember,
    #[derivative(Debug = "ignore")]
    #[serde(deserialize_with = "deserialize_liquid_template_with_custom_filter")]
    pub(crate) test_suite: liquid::Template,
    pub(crate) open: Option<String>,
    pub(crate) template: CargoCompeteConfigTempate,
    pub(crate) submit_via_binary: Option<CargoCompeteConfigSubmitViaBinary>,
}

fn deserialize_liquid_template_with_custom_filter<'de, D>(
    deserializer: D,
) -> Result<liquid::Template, D::Error>
where
    D: Deserializer<'de>,
{
    liquid_template_with_custom_filter(&String::deserialize(deserializer)?)
        .map_err(D::Error::custom)
}

fn liquid_template_with_custom_filter(text: &str) -> Result<liquid::Template, String> {
    use liquid::ParserBuilder;
    use liquid_core::{Filter, Runtime, Value, ValueView};
    use liquid_derive::{Display_filter, FilterReflection, ParseFilter};

    return ParserBuilder::with_stdlib()
        .filter(Kebabcase)
        .build()
        .map_err(|e| e.to_string())?
        .parse(text)
        .map_err(|e| e.to_string());

    #[derive(Clone, ParseFilter, FilterReflection)]
    #[filter(
        name = "kebabcase",
        description = "Returns the absolute value of a number.",
        parsed(KebabcaseFilter) // A struct that implements `Filter` (must implement `Default`)
   )]
    struct Kebabcase;

    #[derive(Default, Debug, Display_filter)]
    #[name = "kebabcase"]
    struct KebabcaseFilter;

    impl Filter for KebabcaseFilter {
        fn evaluate(&self, input: &dyn ValueView, _: &Runtime<'_>) -> liquid_core::Result<Value> {
            Ok(Value::scalar(input.to_kstr().to_kebab_case()))
        }
    }
}

#[derive(Deserialize, Clone, Copy, Debug)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum NewWorkspaceMember {
    Skip,
    Include,
    Exclude,
    Focus,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct CargoCompeteConfigTempate {
    #[serde(deserialize_with = "deserialize_platform_kind_in_kebab_case")]
    pub(crate) platform: PlatformKind,
    pub(crate) manifest: PathBuf,
    pub(crate) src: PathBuf,
}

fn deserialize_platform_kind_in_kebab_case<'de, D>(
    deserializer: D,
) -> Result<PlatformKind, D::Error>
where
    D: Deserializer<'de>,
{
    return PlatformKindKebabCased::deserialize(deserializer).map(|kind| match kind {
        PlatformKindKebabCased::Atcoder => PlatformKind::Atcoder,
        PlatformKindKebabCased::Codeforces => PlatformKind::Codeforces,
        PlatformKindKebabCased::Yukicoder => PlatformKind::Yukicoder,
    });

    #[derive(Deserialize)]
    #[serde(rename_all = "kebab-case")]
    enum PlatformKindKebabCased {
        Atcoder,
        Codeforces,
        Yukicoder,
    }
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct CargoCompeteConfigSubmitViaBinary {
    pub(crate) target: String,
    pub(crate) cross: Option<PathBuf>,
    pub(crate) strip: Option<PathBuf>,
    pub(crate) upx: Option<PathBuf>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct PackageMetadataCargoCompete {
    pub(crate) bin: IndexMap<String, PackageMetadataCargoCompeteBin>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct PackageMetadataCargoCompeteBin {
    pub(crate) name: String,
    pub(crate) problem: TargetProblem,
}

#[derive(Deserialize, Debug, Ord, PartialOrd, Eq, PartialEq)]
#[serde(rename_all = "kebab-case", tag = "platform")]
pub(crate) enum TargetProblem {
    Atcoder {
        contest: String,
        index: String,
        url: Option<Url>,
    },
    Codeforces {
        contest: String,
        index: String,
        url: Option<Url>,
    },
    Yukicoder(TargetProblemYukicoder),
}

impl TargetProblem {
    pub(crate) fn url(&self) -> Option<&Url> {
        match self {
            Self::Atcoder { url, .. }
            | Self::Codeforces { url, .. }
            | Self::Yukicoder(TargetProblemYukicoder::Problem { url, .. })
            | Self::Yukicoder(TargetProblemYukicoder::Contest { url, .. }) => url.as_ref(),
        }
    }
}

#[derive(Deserialize, Debug, Ord, PartialOrd, Eq, PartialEq)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub(crate) enum TargetProblemYukicoder {
    Problem {
        no: u64,
        url: Option<Url>,
    },
    Contest {
        contest: String,
        index: String,
        url: Option<Url>,
    },
}

#[ext(MetadataExt)]
impl Metadata {
    pub(crate) fn read_compete_toml(&self) -> anyhow::Result<CargoCompeteConfig> {
        let path = self.workspace_root.join("compete.toml");
        crate::fs::read_toml(path)
    }

    pub(crate) fn all_members(&self) -> Vec<&Package> {
        self.packages
            .iter()
            .filter(|Package { id, .. }| self.workspace_members.contains(id))
            .collect()
    }

    pub(crate) fn query_for_member<'a, S: AsRef<str>>(
        &'a self,
        spec: Option<S>,
    ) -> anyhow::Result<&'a Package> {
        let spec = spec.as_ref().map(AsRef::as_ref);

        let cargo_exe = env::var_os("CARGO").with_context(|| "`$CARGO` should be present")?;

        let manifest_path = self
            .resolve
            .as_ref()
            .and_then(|Resolve { root, .. }| root.as_ref())
            .map(|id| self[id].manifest_path.clone())
            .unwrap_or_else(|| self.workspace_root.join("Cargo.toml"));

        let output = std::process::Command::new(cargo_exe)
            .arg("pkgid")
            .arg("--manifest-path")
            .arg(manifest_path)
            .args(spec)
            .output()?;
        let stdout = str::from_utf8(&output.stdout)?.trim_end();
        let stderr = str::from_utf8(&output.stderr)?.trim_end();
        if !output.status.success() {
            bail!("{}", stderr.trim_start_matches("error: "));
        }

        let url = stdout.parse::<Url>()?;
        let fragment = url.fragment().expect("the URL should contain fragment");
        let name = match *fragment.splitn(2, ':').collect::<Vec<_>>() {
            [name, _] => name,
            [_] => url
                .path_segments()
                .and_then(Iterator::last)
                .expect("should contain name"),
            _ => unreachable!(),
        };

        self.packages
            .iter()
            .filter(move |Package { id, .. }| self.workspace_members.contains(id))
            .find(|p| p.name == name)
            .with_context(|| {
                let spec = spec.expect("should be present here");
                format!("`{}` is not a member of the workspace", spec)
            })
    }

    pub(crate) fn add_member(
        self,
        package_name: &str,
        problems: &BTreeMap<&str, &Url>,
        problems_are_yukicoder_no: bool,
        shell: &mut Shell,
    ) -> anyhow::Result<Vec<PathBuf>> {
        let cargo_compete_config = self.read_compete_toml()?;

        let mut package_metadata_cargo_compete_bin = problems
            .keys()
            .map(|problem_index| {
                format!(
                    r#"{} = {{ name = "", problem = {{ {} }} }}
"#,
                    escape_key(&problem_index.to_kebab_case()),
                    match (
                        cargo_compete_config.template.platform,
                        problems_are_yukicoder_no
                    ) {
                        (PlatformKind::Atcoder, _) | (PlatformKind::Codeforces, _) => {
                            r#"platform = "", contest = "", index = "", url = """#
                        }
                        (PlatformKind::Yukicoder, true) => {
                            r#"platform = "", kind = "no", no = "", url = """#
                        }
                        (PlatformKind::Yukicoder, false) => {
                            r#"platform = "", kind = "contest", contest = "", index = "", url = """#
                        }
                    }
                )
            })
            .join("")
            .parse::<toml_edit::Document>()?;

        for (problem_index, problem_url) in problems {
            package_metadata_cargo_compete_bin[&problem_index.to_kebab_case()]["name"] =
                toml_edit::value(format!(
                    "{}-{}",
                    package_name,
                    problem_index.to_kebab_case(),
                ));

            let tbl =
                &mut package_metadata_cargo_compete_bin[&problem_index.to_kebab_case()]["problem"];

            match cargo_compete_config.template.platform {
                PlatformKind::Atcoder => {
                    tbl["platform"] = toml_edit::value("atcoder");
                    tbl["contest"] = toml_edit::value(package_name);
                    tbl["index"] = toml_edit::value(&**problem_index);
                    tbl["url"] = toml_edit::value(problem_url.as_str());
                }
                PlatformKind::Codeforces => {
                    tbl["platform"] = toml_edit::value("codeforces");
                    tbl["contest"] = toml_edit::value(package_name);
                    tbl["index"] = toml_edit::value(&**problem_index);
                    tbl["url"] = toml_edit::value(problem_url.as_str());
                }
                PlatformKind::Yukicoder => {
                    tbl["platform"] = toml_edit::value("yukicoder");
                    if problems_are_yukicoder_no {
                        tbl["no"] = toml_edit::value(&**problem_index);
                    } else {
                        tbl["contest"] = toml_edit::value(package_name);
                        tbl["index"] = toml_edit::value(&**problem_index);
                    }
                    tbl["url"] = toml_edit::value(problem_url.as_str());
                }
            }
        }

        let template_manifest_path = self
            .workspace_root
            .join(&cargo_compete_config.template.manifest);

        let mut manifest = crate::fs::read_to_string(&template_manifest_path)?
            .parse::<toml_edit::Document>()
            .with_context(|| {
                format!(
                    "could not parse the manifest at `{}`",
                    template_manifest_path.display(),
                )
            })?;

        manifest["package"]["name"] = toml_edit::value(package_name);

        set_implicit_table_if_none(&mut manifest["package"]["metadata"]);
        set_implicit_table_if_none(&mut manifest["package"]["metadata"]["cargo-compete"]);
        set_implicit_table_if_none(&mut manifest["package"]["metadata"]["cargo-compete"]["bin"]);

        for (key, val) in package_metadata_cargo_compete_bin.as_table().iter() {
            manifest["package"]["metadata"]["cargo-compete"]["bin"][key] = val.clone();
        }

        if let Ok(new_manifest) = manifest
            .to_string()
            .replace("\"} }", "\" } }")
            .parse::<toml_edit::Document>()
        {
            manifest = new_manifest;
        }

        manifest["bin"] = toml_edit::Item::ArrayOfTables({
            let mut arr = toml_edit::ArrayOfTables::new();
            for problem_index in problems.keys() {
                let mut tbl = toml_edit::Table::new();
                tbl["name"] = toml_edit::value(format!(
                    "{}-{}",
                    package_name,
                    problem_index.to_kebab_case(),
                ));
                tbl["path"] =
                    toml_edit::value(format!("src/bin/{}.rs", problem_index.to_kebab_case()));
                arr.append(tbl);
            }
            arr
        });

        let pkg_manifest_dir = self.workspace_root.join(package_name);

        if pkg_manifest_dir.exists() {
            bail!("`{}` already exists", pkg_manifest_dir.display());
        }
        crate::fs::create_dir_all(&pkg_manifest_dir)?;

        let pkg_manifest_path = pkg_manifest_dir.join("Cargo.toml");
        crate::fs::write(&pkg_manifest_path, manifest.to_string())?;

        let src_bin = pkg_manifest_dir.join("src").join("bin");
        crate::fs::create_dir_all(&src_bin)?;

        let template_code =
            crate::fs::read_to_string(self.workspace_root.join(cargo_compete_config.template.src))?;

        let src_paths = problems
            .keys()
            .map(|problem_index| {
                src_bin
                    .join(problem_index.to_kebab_case())
                    .with_extension("rs")
            })
            .collect::<Vec<_>>();

        for src_path in &src_paths {
            crate::fs::write(src_path, &template_code)?;
        }

        shell.status(
            "Created",
            format!(
                "`{}` package at {}",
                package_name,
                pkg_manifest_dir.display()
            ),
        )?;

        match cargo_compete_config.new_workspace_member {
            NewWorkspaceMember::Skip => {}
            NewWorkspaceMember::Include => {
                cargo_member::Include::new(&self.workspace_root, &[pkg_manifest_dir])
                    .stderr(shell.err())
                    .exec()?;
            }
            NewWorkspaceMember::Exclude => {
                cargo_member::Exclude::new(&self.workspace_root, &[&pkg_manifest_dir])
                    .stderr(shell.err())
                    .exec()?;
                let dst = symlink_compete_toml(&self.workspace_root, &pkg_manifest_dir)?;
                shell.status("Created", format!("a symlink at {}", dst.display()))?;
            }
            NewWorkspaceMember::Focus => {
                cargo_member::Focus::new(&self.workspace_root, &pkg_manifest_dir)
                    .stderr(shell.err())
                    .exec()?;
            }
        }

        return Ok(src_paths);

        fn escape_key(s: &str) -> String {
            if s.chars().any(|c| c.is_whitespace() || c.is_control()) {
                return toml::Value::String(s.to_owned()).to_string();
            }

            let mut doc = toml_edit::Document::new();
            doc[s] = toml_edit::value(0);
            doc.to_string()
                .trim_end()
                .trim_end_matches('0')
                .trim_end()
                .trim_end_matches('=')
                .trim_end()
                .to_owned()
        }

        fn set_implicit_table_if_none(item: &mut toml_edit::Item) {
            if item.is_none() {
                *item = {
                    let mut tbl = toml_edit::Table::new();
                    tbl.set_implicit(true);
                    toml_edit::Item::Table(tbl)
                };
            }
        }
    }
}

fn symlink_compete_toml(workspace_root: &Path, pkg_manifest_dir: &Path) -> anyhow::Result<PathBuf> {
    #[cfg(unix)]
    use std::os::unix::fs::symlink as symlink_file;

    #[cfg(windows)]
    use std::os::windows::fs::symlink_file;

    let src = if let Ok(path) = pkg_manifest_dir.strip_prefix(workspace_root) {
        path.iter()
            .fold(PathBuf::new(), |p, _| p.join(".."))
            .join("compete.toml")
    } else {
        unimplemented!()
    };
    let dst = pkg_manifest_dir.join("compete.toml");

    symlink_file(&src, &dst).with_context(|| {
        format!(
            "could not create symlink: `{}` -> `{}`",
            src.display(),
            dst.display(),
        )
    })?;
    Ok(dst)
}

#[ext(PackageExt)]
impl Package {
    pub(crate) fn manifest_dir(&self) -> &Path {
        self.manifest_path
            .parent()
            .expect("`manifest_path` should end with `Cargo.toml`")
    }

    pub(crate) fn manifest_dir_utf8(&self) -> &str {
        self.manifest_dir()
            .to_str()
            .expect("this is from JSON string")
    }

    pub(crate) fn read_package_metadata(&self) -> anyhow::Result<PackageMetadataCargoCompete> {
        let CargoToml {
            package:
                CargoTomlPackage {
                    metadata: CargoTomlPackageMetadata { cargo_compete },
                },
        } = crate::fs::read_toml(&self.manifest_path)?;
        return Ok(cargo_compete);

        #[derive(Deserialize)]
        #[serde(rename_all = "kebab-case")]
        struct CargoToml {
            package: CargoTomlPackage,
        }

        #[derive(Deserialize)]
        #[serde(rename_all = "kebab-case")]
        struct CargoTomlPackage {
            metadata: CargoTomlPackageMetadata,
        }

        #[derive(Deserialize)]
        #[serde(rename_all = "kebab-case")]
        struct CargoTomlPackageMetadata {
            cargo_compete: PackageMetadataCargoCompete,
        }
    }

    pub(crate) fn bin_target<'a>(&'a self, name: &str) -> anyhow::Result<&'a Target> {
        self.targets
            .iter()
            .find(|t| t.name == name && t.kind == ["bin".to_owned()])
            .with_context(|| format!("no bin target named `{}` in `{}`", name, self.name))
    }

    pub(crate) fn all_bin_targets_sorted(&self) -> Vec<&Target> {
        self.targets
            .iter()
            .filter(|Target { kind, .. }| *kind == ["bin".to_owned()])
            .sorted_by(|t1, t2| t1.name.cmp(&t2.name))
            .collect()
    }
}

pub(crate) fn locate_project(cwd: PathBuf) -> anyhow::Result<PathBuf> {
    cwd.ancestors()
        .map(|p| p.join("Cargo.toml"))
        .find(|p| p.exists())
        .with_context(|| {
            format!(
                "could not find `Cargo.toml` in `{}` or any parent directory. first, run \
                 `cargo compete init` and `cd` to a workspace",
                cwd.display(),
            )
        })
}

pub(crate) fn cargo_metadata(manifest_path: impl AsRef<Path>) -> cargo_metadata::Result<Metadata> {
    MetadataCommand::new()
        .manifest_path(manifest_path.as_ref())
        .exec()
}

pub(crate) fn cargo_metadata_no_deps_frozen(
    manifest_path: impl AsRef<Path>,
) -> cargo_metadata::Result<Metadata> {
    MetadataCommand::new()
        .manifest_path(manifest_path.as_ref())
        .no_deps()
        .other_options(vec!["--frozen".to_owned()])
        .exec()
}

pub(crate) fn gen_compete_toml(
    platform: PlatformKind,
    submit_via_binary: bool,
) -> Result<String, liquid::Error> {
    liquid::ParserBuilder::with_stdlib()
        .build()?
        .parse(include_str!("../resources/compete.toml.liquid"))?
        .render(&object!({
            "template_platform": platform.to_kebab_case_str(),
            "submit_via_binary": submit_via_binary,
        }))
}

pub(crate) fn new_template_package(
    workspace_root: &Path,
    deps: Option<&str>,
    main_rs: &str,
    shell: &mut Shell,
) -> anyhow::Result<()> {
    let new_pkg_manifest_dir = workspace_root.join("cargo-compete-template");
    let new_pkg_manifest_path = new_pkg_manifest_dir.join("Cargo.toml");

    crate::fs::create_dir_all(&new_pkg_manifest_dir)?;

    let mut new_manifest = r#"[package]
name = "cargo-compete-template"
version = "0.1.0"
edition = "2018"
publish = false

[[bin]]
name = "cargo-compete-template"
path = "src/main.rs"
"#
    .to_owned();

    if let Some(deps) = deps {
        new_manifest += "\n";
        new_manifest += "[dependencies]\n";
        new_manifest += deps;
    }

    crate::fs::write(&new_pkg_manifest_path, new_manifest)?;
    crate::fs::create_dir_all(new_pkg_manifest_dir.join("src"))?;
    crate::fs::write(new_pkg_manifest_dir.join("src").join("main.rs"), main_rs)?;
    shell.status(
        "Created",
        format!(
            "`cargo-compete-template` package at {}",
            new_pkg_manifest_dir.display(),
        ),
    )?;

    shell.status("Updating", workspace_root.join("Cargo.lock").display())?;

    MetadataCommand::new()
        .manifest_path(new_pkg_manifest_path)
        .exec()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::project::CargoCompeteConfig;
    use liquid::object;
    use pretty_assertions::assert_eq;
    use snowchains_core::web::PlatformKind;

    #[test]
    fn liquid_template_with_custom_filter() -> anyhow::Result<()> {
        let output = super::liquid_template_with_custom_filter("{{ s | kebabcase }}")
            .map_err(anyhow::Error::msg)?
            .render(&object!({ "s": "FooBarBaz" }))?;
        assert_eq!("foo-bar-baz", output);
        Ok(())
    }

    #[test]
    fn symlink_compete_toml() -> anyhow::Result<()> {
        let tempdir = tempfile::tempdir()?;

        std::fs::write(tempdir.path().join("compete.toml"), "#content\n")?;
        std::fs::create_dir(tempdir.path().join("a"))?;

        let dst = super::symlink_compete_toml(tempdir.path(), &tempdir.path().join("a"))?;
        assert_eq!(tempdir.path().join("a").join("compete.toml"), dst);
        assert_eq!(std::fs::read_to_string(dst)?, "#content\n");

        tempdir.close()?;
        Ok(())
    }

    #[test]
    fn gen_compete_toml() -> anyhow::Result<()> {
        fn test(platform: PlatformKind, submit_via_binary: bool) -> anyhow::Result<()> {
            let content = super::gen_compete_toml(platform, submit_via_binary)?;
            toml::from_str::<CargoCompeteConfig>(&content)?;
            Ok(())
        }

        test(PlatformKind::Atcoder, false)?;
        test(PlatformKind::Atcoder, true)?;
        test(PlatformKind::Codeforces, false)?;
        test(PlatformKind::Yukicoder, false)
    }
}
