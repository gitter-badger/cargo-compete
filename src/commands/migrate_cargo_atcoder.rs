use crate::{
    project::{MetadataExt as _, PackageExt as _},
    shell::ColorChoice,
};
use anyhow::Context as _;
use cargo_metadata::Package;
use if_chain::if_chain;
use ignore::{overrides::OverrideBuilder, WalkBuilder};
use itertools::Itertools as _;
use snowchains_core::web::PlatformKind;
use std::{iter, path::PathBuf};
use structopt::StructOpt;
use strum::VariantNames as _;
use termcolor::Color;

#[derive(StructOpt, Debug)]
pub struct OptCompeteMigrateCargoAtcoder {
    /// Process glob patterns given with the `--glob` flag case insensitively
    #[structopt(long)]
    pub glob_case_insensitive: bool,

    /// Include or exclude manifest paths. For more detail, see the help of ripgrep
    #[structopt(short, long, value_name("GLOB"))]
    pub glob: Vec<String>,

    /// Coloring
    #[structopt(
        long,
        value_name("WHEN"),
        possible_values(ColorChoice::VARIANTS),
        default_value("auto")
    )]
    pub color: ColorChoice,

    #[structopt(default_value("."))]
    pub path: PathBuf,
}

pub(crate) fn run(
    opt: OptCompeteMigrateCargoAtcoder,
    ctx: crate::Context<'_>,
) -> anyhow::Result<()> {
    let OptCompeteMigrateCargoAtcoder {
        glob_case_insensitive,
        glob,
        color,
        path,
    } = opt;

    let crate::Context {
        cwd,
        cookies_path: _,
        shell,
    } = ctx;

    shell.set_color_choice(color);

    let path = cwd.join(path.strip_prefix(".").unwrap_or(&path));

    let manifest_paths = WalkBuilder::new(&path)
        .follow_links(true)
        .max_depth(Some(32))
        .overrides({
            let mut overrides = OverrideBuilder::new(&path);
            for glob in glob {
                overrides.add(&glob)?;
            }
            overrides.case_insensitive(glob_case_insensitive)?.build()?
        })
        .build()
        .map(|entry| {
            let manifest_path = entry?.into_path();
            Ok(
                if manifest_path.file_name() == Some("Cargo.toml".as_ref()) {
                    Some(manifest_path)
                } else {
                    None
                },
            )
        })
        .flat_map(Result::transpose)
        .collect::<Result<Vec<_>, ignore::Error>>()?;

    let mut include = vec![];

    for manifest_path in manifest_paths.into_iter().sorted() {
        let metadata = crate::project::cargo_metadata_no_deps_frozen(&manifest_path)?;
        if_chain! {
            if let [package] = *metadata.all_members();
            if package.manifest_path == manifest_path;
            then {
                shell.status("Found", format_args!("`{}`", manifest_path.display()))?;
                include.push(package.clone());
            } else {
                shell.status_with_color(
                    "Ignoring",
                    format_args!("`{}`", manifest_path.display()),
                    Color::Cyan,
                )?;
            }
        }
    }

    for package in &include {
        let mut manifest =
            crate::fs::read_to_string(&package.manifest_path)?.parse::<toml_edit::Document>()?;

        manifest["profile"] = toml_edit::Item::None;

        let bins = package.all_bin_targets_sorted();

        if manifest["package"]["metadata"]["cargo-compete"].is_none() {
            manifest["package"]["metadata"] = implicit_table();
            manifest["package"]["metadata"]["cargo-compete"] = implicit_table();
            manifest["package"]["metadata"]["cargo-compete"]["bin"] = toml_edit::Item::Table({
                let mut tbl = toml_edit::Table::new();
                for bin in &bins {
                    tbl[&bin.name]["name"] =
                        toml_edit::value(format!("{}-{}", package.name, bin.name));
                    tbl[&bin.name]["problem"]["platform"] = toml_edit::value("atcoder");
                    tbl[&bin.name]["problem"]["contest"] = toml_edit::value(&*package.name);
                    tbl[&bin.name]["problem"]["index"] = toml_edit::value(bin.name.to_uppercase());
                }
                tbl
            });
        }

        if manifest["bin"].is_none() {
            manifest["bin"] = toml_edit::Item::ArrayOfTables({
                let mut arr = toml_edit::ArrayOfTables::new();
                for bin in bins {
                    let mut tbl = toml_edit::Table::new();
                    tbl["name"] = toml_edit::value(format!("{}-{}", package.name, bin.name));
                    tbl["path"] = toml_edit::value(format!("./src/bin/{}.rs", bin.name));
                    arr.append(tbl);
                }
                arr
            });
        }

        crate::fs::write(&package.manifest_path, manifest.to_string())?;
        shell.status("Wrote", package.manifest_path.display())?;
    }

    for package in &include {
        let lock_path = package.manifest_path.with_file_name("Cargo.lock");
        if lock_path.exists() {
            crate::fs::remove_file(&lock_path)?;
            shell.status("Removed", lock_path.display())?;
        }
    }

    crate::project::new_template_package(
        &path,
        None,
        include_str!("../../resources/template-main.rs"),
        shell,
    )?;

    let cargo_atcoder_config = (|| -> _ {
        let path = dirs::config_dir()?.join("cargo-atcoder.toml");
        crate::fs::read_to_string(path)
            .ok()?
            .parse::<toml_edit::Document>()
            .ok()
    })();

    let submit_via_binary = matches!(
        &cargo_atcoder_config,
        Some(c) if c["atcoder"]["submit_via_binary"].as_bool() == Some(true)
    );

    let mut root_manifest = r#"[workspace]
members = []
exclude = []
"#
    .parse::<toml_edit::Document>()
    .unwrap();

    if submit_via_binary {
        if let Some(profile_release) = (|| -> _ {
            let path = dirs::config_dir()?.join("cargo-atcoder.toml");
            let config = crate::fs::read_to_string(path)
                .ok()?
                .parse::<toml_edit::Document>()
                .ok()?;
            Some(config["profile"]["release"].clone())
        })() {
            root_manifest["profile"] = implicit_table();
            root_manifest["profile"]["release"] = profile_release;
        }
    }

    let root_manifest_path = path.join("Cargo.toml");
    crate::fs::write(&root_manifest_path, root_manifest.to_string())?;
    shell.status("Wrote", root_manifest_path.display())?;

    shell.status("Adding", format!("{} + 1 packages", include.len()))?;

    cargo_member::Include::new(
        &path,
        include
            .iter()
            .map(|Package { manifest_path, .. }| manifest_path.with_file_name(""))
            .chain(iter::once(path.join("cargo-compete-template"))),
    )
    .stderr(shell.err())
    .exec()
    .with_context(|| {
        "could not migrate. Run `git clean -f && git restore .`, and this command again with \
         `--glob` option"
    })?;

    let compete_toml_path = path.join("compete.toml");
    let compete_toml = crate::project::gen_compete_toml(PlatformKind::Atcoder, submit_via_binary)?;
    crate::fs::write(&compete_toml_path, compete_toml)?;
    shell.status("Wrote", compete_toml_path.display())?;

    shell.status("Finished", "migrating")?;

    if include.len() >= 100 {
        shell.warn(
            "too many packages. install `cargo-member` and run `cargo member focus`, and set \
             `new-workspace-member` in the `compete.toml` to `focus`.",
        )?;
    }

    Ok(())
}

fn implicit_table() -> toml_edit::Item {
    let mut tbl = toml_edit::Table::new();
    tbl.set_implicit(true);
    toml_edit::Item::Table(tbl)
}
