//! The rehearsal plan, read from a TOML file, and the Zebra config built from it.

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Height where `upgrade.previous` activates. Zebra activates the upgrades
/// before it at the same height or lower.
const PREVIOUS_HEIGHT: u32 = 2;

/// Transparent regtest address that receives block rewards. Same default as Z3.
const DEFAULT_MINER_ADDRESS: &str = "tmSRd1r8gs77Ja67Fw1JcdoXytxsyrLTPJm";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub name: String,
    pub node: NodeSpec,
    pub upgrade: UpgradeSpec,
    #[serde(default, rename = "project")]
    pub projects: Vec<Project>,
    /// Directory of the plan file. Project commands run from here.
    #[serde(skip)]
    pub base_dir: PathBuf,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeSpec {
    pub image: String,
    #[serde(default = "default_miner_address")]
    pub miner_address: String,
    #[serde(default = "default_ready_timeout")]
    pub ready_timeout_secs: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpgradeSpec {
    pub name: String,
    /// The upgrade active before `name`. Empty when the plan leaves it out.
    #[serde(default)]
    pub previous: String,
    pub height: u32,
    #[serde(default = "default_blocks_after")]
    pub blocks_after: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub name: String,
    /// Shell command, run with `sh -c` after the upgrade is active.
    pub run: String,
    #[serde(default = "default_project_timeout")]
    pub timeout_secs: u64,
}

fn default_miner_address() -> String {
    DEFAULT_MINER_ADDRESS.to_string()
}
fn default_ready_timeout() -> u64 {
    120
}
fn default_blocks_after() -> u32 {
    10
}
fn default_project_timeout() -> u64 {
    600
}

impl Plan {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading plan {}", path.display()))?;
        let mut plan: Plan =
            toml::from_str(&text).with_context(|| format!("parsing plan {}", path.display()))?;
        plan.base_dir = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .to_path_buf();
        plan.validate()?;
        Ok(plan)
    }

    fn validate(&self) -> Result<()> {
        let u = &self.upgrade;
        if u.name.is_empty() {
            bail!("upgrade.name must not be empty");
        }
        if u.previous.is_empty() {
            bail!(
                "plan needs upgrade.previous, the upgrade active before `{}`",
                u.name
            );
        }
        if u.previous == u.name {
            bail!(
                "upgrade.previous must differ from upgrade.name `{}`",
                u.name
            );
        }
        if u.height <= PREVIOUS_HEIGHT {
            bail!(
                "upgrade height {} must be above {PREVIOUS_HEIGHT}, where `{}` activates",
                u.height,
                u.previous
            );
        }
        let mut seen = std::collections::HashSet::new();
        for p in &self.projects {
            if !seen.insert(p.name.as_str()) {
                bail!("project name {:?} is used twice", p.name);
            }
            if p.name.is_empty()
                || !p
                    .name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_".contains(c))
            {
                bail!(
                    "project name {:?} may only use letters, digits, '-' and '_'",
                    p.name
                );
            }
        }
        Ok(())
    }

    /// The `zebrad.toml` for the rehearsal node. `previous` activates early
    /// and the target at the planned height. Zebra fills in the upgrades
    /// before `previous`, and later upgrades are left out so they never
    /// activate.
    pub fn zebrad_toml(&self) -> String {
        let key = |name: &str| toml::Value::String(name.to_string()).to_string();
        format!(
            "[network]\nnetwork = \"Regtest\"\n\n[network.testnet_parameters.activation_heights]\n{} = {PREVIOUS_HEIGHT}\n{} = {}\n",
            key(&self.upgrade.previous),
            key(&self.upgrade.name),
            self.upgrade.height
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(toml: &str) -> Result<Plan> {
        let mut p: Plan = toml::from_str(toml)?;
        p.base_dir = PathBuf::from(".");
        p.validate()?;
        Ok(p)
    }

    const NODE: &str = "[node]\nimage = \"zfnd/zebra:6.2.3\"\n";

    fn with_upgrade(upgrade: &str) -> Result<Plan> {
        plan(&format!("name = \"t\"\n{NODE}[upgrade]\n{upgrade}"))
    }

    #[test]
    fn zebrad_toml_sets_only_previous_and_target() {
        let p = with_upgrade("name = \"NU6.3\"\nprevious = \"NU6.2\"\nheight = 20\n").unwrap();
        assert_eq!(
            p.zebrad_toml(),
            "[network]\nnetwork = \"Regtest\"\n\n[network.testnet_parameters.activation_heights]\n\
             \"NU6.2\" = 2\n\"NU6.3\" = 20\n"
        );
    }

    #[test]
    fn zebrad_toml_escapes_names() {
        let p = with_upgrade("name = 'a\"b'\nprevious = \"NU6.2\"\nheight = 20\n").unwrap();
        let cfg: toml::Table = toml::from_str(&p.zebrad_toml()).unwrap();
        let heights = &cfg["network"]["testnet_parameters"]["activation_heights"];
        assert_eq!(heights["a\"b"].as_integer(), Some(20));
    }

    #[test]
    fn rejects_bad_upgrades() {
        let err = |u: &str| with_upgrade(u).unwrap_err().to_string();
        assert_eq!(
            err("name = \"NU6.3\"\nheight = 20\n"),
            "plan needs upgrade.previous, the upgrade active before `NU6.3`"
        );
        assert!(
            err("name = \"NU6.3\"\nprevious = \"NU6.3\"\nheight = 20\n").contains("must differ")
        );
        assert!(
            err("name = \"\"\nprevious = \"NU6.2\"\nheight = 20\n").contains("must not be empty")
        );
        assert!(
            err("name = \"NU6.3\"\nprevious = \"NU6.2\"\nheight = 2\n").contains("must be above 2")
        );
        assert!(with_upgrade("name = \"NU6.3\"\nprevious = \"NU6.2\"\nheight = 3\n").is_ok());
    }

    #[test]
    fn rejects_duplicate_and_unsafe_project_names() {
        let upgrade = "name = \"NU6.3\"\nprevious = \"NU6.2\"\nheight = 5\n";
        let dup = format!(
            "{upgrade}[[project]]\nname = \"a\"\nrun = \"true\"\n[[project]]\nname = \"a\"\nrun = \"true\"\n"
        );
        assert!(
            with_upgrade(&dup)
                .unwrap_err()
                .to_string()
                .contains("used twice")
        );
        let unsafe_name = format!("{upgrade}[[project]]\nname = \"../x\"\nrun = \"true\"\n");
        assert!(with_upgrade(&unsafe_name).is_err());
    }

    #[test]
    fn defaults_fill_in() {
        let p = with_upgrade("name = \"NU6.3\"\nprevious = \"NU6.2\"\nheight = 5\n").unwrap();
        assert_eq!(p.upgrade.blocks_after, 10);
        assert_eq!(p.node.miner_address, DEFAULT_MINER_ADDRESS);
        assert!(p.projects.is_empty());
    }
}
