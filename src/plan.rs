//! The rehearsal plan, read from a TOML file, and the Zebra config built from it.

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Network upgrades in activation order, with the regtest height each one
/// gets when it activates *before* the upgrade under rehearsal. Same heights
/// Z3 uses for its regtest stack. A new upgrade (NU7) is one more line here,
/// once the Zebra image under test knows its name.
pub const UPGRADES: &[(&str, u32)] = &[
    ("BeforeOverwinter", 1),
    ("Overwinter", 1),
    ("Sapling", 1),
    ("Blossom", 1),
    ("Heartwood", 1),
    ("Canopy", 1),
    ("NU5", 2),
    ("NU6", 2),
    ("NU6.1", 2),
    ("NU6.2", 2),
    ("NU6.3", 2),
];

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
        let idx = upgrade_index(&self.upgrade.name)?;
        // Every earlier upgrade must already be active when the target arrives.
        let floor = UPGRADES[..idx].iter().map(|(_, h)| *h).max().unwrap_or(0);
        if self.upgrade.height <= floor {
            bail!(
                "upgrade height {} must be above {floor}, where the earlier upgrades activate",
                self.upgrade.height
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

    /// The `zebrad.toml` for the rehearsal node. Upgrades before the target
    /// activate early, the target activates at the planned height, and later
    /// upgrades are left out so they never activate.
    pub fn zebrad_toml(&self) -> String {
        let idx = upgrade_index(&self.upgrade.name).expect("validated in load");
        let mut out = String::from(
            "[network]\nnetwork = \"Regtest\"\n\n[network.testnet_parameters.activation_heights]\n",
        );
        for (name, height) in &UPGRADES[..idx] {
            out.push_str(&format!("\"{name}\" = {height}\n"));
        }
        out.push_str(&format!(
            "\"{}\" = {}\n",
            self.upgrade.name, self.upgrade.height
        ));
        out
    }
}

fn upgrade_index(name: &str) -> Result<usize> {
    UPGRADES
        .iter()
        .position(|(n, _)| *n == name)
        .with_context(|| {
            let known: Vec<_> = UPGRADES.iter().map(|(n, _)| *n).collect();
            format!("unknown upgrade {name:?}; known: {}", known.join(", "))
        })
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

    #[test]
    fn zebrad_toml_stops_at_the_target_upgrade() {
        let p = plan(&format!(
            "name = \"t\"\n{NODE}[upgrade]\nname = \"NU6.1\"\nheight = 20\n"
        ))
        .unwrap();
        let cfg = p.zebrad_toml();
        assert!(cfg.contains("\"NU6\" = 2\n"));
        assert!(cfg.contains("\"NU6.1\" = 20\n"));
        assert!(
            !cfg.contains("NU6.2"),
            "later upgrades must not activate:\n{cfg}"
        );
        assert!(cfg.contains("network = \"Regtest\""));
    }

    #[test]
    fn rejects_unknown_upgrade_and_low_height() {
        let unknown = plan(&format!(
            "name = \"t\"\n{NODE}[upgrade]\nname = \"NU9\"\nheight = 20\n"
        ));
        assert!(unknown.unwrap_err().to_string().contains("unknown upgrade"));
        let low = plan(&format!(
            "name = \"t\"\n{NODE}[upgrade]\nname = \"NU6.3\"\nheight = 2\n"
        ));
        assert!(low.unwrap_err().to_string().contains("must be above 2"));
    }

    #[test]
    fn rejects_duplicate_and_unsafe_project_names() {
        let dup = format!(
            "name = \"t\"\n{NODE}[upgrade]\nname = \"NU6.3\"\nheight = 5\n\
             [[project]]\nname = \"a\"\nrun = \"true\"\n[[project]]\nname = \"a\"\nrun = \"true\"\n"
        );
        assert!(plan(&dup).unwrap_err().to_string().contains("used twice"));
        let unsafe_name = format!(
            "name = \"t\"\n{NODE}[upgrade]\nname = \"NU6.3\"\nheight = 5\n[[project]]\nname = \"../x\"\nrun = \"true\"\n"
        );
        assert!(plan(&unsafe_name).is_err());
    }

    #[test]
    fn defaults_fill_in() {
        let p = plan(&format!(
            "name = \"t\"\n{NODE}[upgrade]\nname = \"NU6.3\"\nheight = 5\n"
        ))
        .unwrap();
        assert_eq!(p.upgrade.blocks_after, 10);
        assert_eq!(p.node.miner_address, DEFAULT_MINER_ADDRESS);
        assert!(p.projects.is_empty());
    }
}
