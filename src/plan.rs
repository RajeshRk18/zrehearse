//! The rehearsal plan, read from a TOML file, and the Zebra config built from it.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Height where `upgrade.previous` activates. Zebra activates the upgrades
/// before it at the same height or lower.
const PREVIOUS_HEIGHT: u32 = 2;

/// Blocks before a transparent coinbase output can be spent, Zebra's
/// `MIN_TRANSPARENT_COINBASE_MATURITY`.
pub const COINBASE_MATURITY: u32 = 100;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub name: String,
    pub node: NodeSpec,
    pub upgrade: UpgradeSpec,
    #[serde(default, rename = "project")]
    pub projects: Vec<Project>,
    pub light_server: Option<LightServerSpec>,
    #[serde(default)]
    pub funding: FundingSpec,
    /// Directory of the plan file. Project commands run from here.
    #[serde(skip)]
    pub base_dir: PathBuf,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeSpec {
    pub image: String,
    #[serde(default = "default_ready_timeout")]
    pub ready_timeout_secs: u64,
    /// Merged into the generated node config, for settings that only one node
    /// implementation or version needs.
    #[serde(default)]
    pub config: toml::Table,
    /// Extra environment variables for the node container.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpgradeSpec {
    pub name: String,
    /// The upgrade active before `name`. Empty when the plan leaves it out.
    #[serde(default)]
    pub previous: String,
    #[serde(default = "default_height")]
    pub height: u32,
    #[serde(default = "default_blocks_after")]
    pub blocks_after: u32,
}

/// zrehearse's public test seed phrase (`hidden` x 23 + `protect`, a valid
/// BIP-39 phrase). Everyone can read it, so never use it for real funds.
pub const PUBLIC_TEST_MNEMONIC: &str = "hidden hidden hidden hidden hidden hidden hidden hidden hidden hidden hidden hidden hidden hidden hidden hidden hidden hidden hidden hidden hidden hidden hidden protect";
/// The default Unified Address of ZIP 32 account 0 of that phrase, on regtest.
/// Diversifier index 0 is not a valid Sapling diversifier for this key, so the
/// default address uses index 1.
pub const PUBLIC_TEST_ADDRESS: &str = "uregtest17eg8tfltj5e6s90d78cd9vwdm87aqf6x5q0rm3cfr45s4ljextrxlh092dec72gv9saccwrxc779k9rakhuaterjvctch3h6sdy83uxxxutefj5kz4w6z8nss7lqx0k63u5auq2ta95wd6lmvzt26puej5pps6py4aj2r0l62gm6c604";

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FundingSpec {
    /// Receives the shielded block rewards. The default is
    /// `PUBLIC_TEST_ADDRESS`, whose seed phrase projects get.
    pub shielded_address: Option<String>,
}

impl FundingSpec {
    pub fn shielded_address(&self) -> &str {
        self.shielded_address
            .as_deref()
            .unwrap_or(PUBLIC_TEST_ADDRESS)
    }

    /// The seed phrase of the shielded address, when zrehearse knows it.
    pub fn mnemonic(&self) -> Option<&str> {
        self.shielded_address
            .is_none()
            .then_some(PUBLIC_TEST_MNEMONIC)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LightServerSpec {
    pub kind: LightServerKind,
    pub image: String,
    /// How long the light server may take to reach each tip.
    #[serde(default = "default_ready_timeout")]
    pub ready_timeout_secs: u64,
    /// Merged into the generated `zainod.toml`. Zaino only.
    #[serde(default)]
    pub config: toml::Table,
    /// Extra environment variables for the light server container.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Extra arguments after the image, for example lightwalletd flags.
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LightServerKind {
    Lightwalletd,
    Zaino,
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

/// Late enough that the funded key's first block rewards are spendable
/// before activation.
fn default_height() -> u32 {
    110
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
        reject_owned_keys("node.config", &self.node.config, NODE_CONFIG_KEYS)?;
        reject_owned_env("node.env", &self.node.env, NODE_ENV_SUFFIXES)?;
        if let Some(ls) = &self.light_server {
            if !ls.config.is_empty() && matches!(ls.kind, LightServerKind::Lightwalletd) {
                bail!(
                    "light_server.config is only for zaino. Use light_server.args for lightwalletd"
                );
            }
            reject_owned_keys("light_server.config", &ls.config, &[&["network"]])?;
            reject_owned_env("light_server.env", &ls.env, ZAINO_ENV_SUFFIXES)?;
            if let Some(arg) = ls.args.iter().find(|a| {
                LIGHTWALLETD_FLAGS
                    .iter()
                    .any(|f| a.as_str() == *f || a.starts_with(&format!("{f}=")))
            }) {
                bail!("light_server.args sets {arg}, which zrehearse owns");
            }
        }
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
        let tip = u.height.saturating_add(u.blocks_after);
        if tip <= COINBASE_MATURITY {
            bail!(
                "upgrade.height + upgrade.blocks_after is {tip}, but it must be above \
                 {COINBASE_MATURITY}. A block reward is spendable only {COINBASE_MATURITY} \
                 blocks later, and the funded key needs one."
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
    /// `node.config` is merged in last.
    pub fn zebrad_toml(&self) -> String {
        let mut heights = toml::Table::new();
        heights.insert(self.upgrade.previous.clone(), PREVIOUS_HEIGHT.into());
        heights.insert(self.upgrade.name.clone(), self.upgrade.height.into());
        let mut config: toml::Table = toml::toml! {
            [network]
            network = "Regtest"
        };
        config["network"]
            .as_table_mut()
            .expect("network is a table")
            .insert(
                "testnet_parameters".into(),
                toml::Value::Table(toml::Table::from_iter([(
                    "activation_heights".to_string(),
                    toml::Value::Table(heights),
                )])),
            );
        merge(&mut config, &self.node.config);
        toml::to_string(&config).expect("a TOML table serializes")
    }
}

/// Config keys that zrehearse writes and its checks depend on.
const NODE_CONFIG_KEYS: &[&[&str]] = &[
    &["network", "network"],
    &["network", "testnet_parameters", "activation_heights"],
    &["rpc", "listen_addr"],
    &["rpc", "enable_cookie_auth"],
    &["mining", "miner_address"],
];
/// Node env vars that zrehearse sets, after the `ZEBRA_` or `ZAKURA_` prefix.
const NODE_ENV_SUFFIXES: &[&str] = &[
    "_RPC__LISTEN_ADDR",
    "_RPC__ENABLE_COOKIE_AUTH",
    "_MINING__MINER_ADDRESS",
];
/// Zaino env vars that zrehearse sets.
const ZAINO_ENV_SUFFIXES: &[&str] = &[
    "_VALIDATOR_SETTINGS__VALIDATOR_JSONRPC_LISTEN_ADDRESS",
    "_GRPC_SETTINGS__LISTEN_ADDRESS",
];
/// lightwalletd flags that zrehearse sets.
const LIGHTWALLETD_FLAGS: &[&str] = &["--grpc-bind-addr", "--rpchost", "--rpcport"];

fn reject_owned_keys(field: &str, table: &toml::Table, owned: &[&[&str]]) -> Result<()> {
    for path in owned {
        let mut at = Some(table);
        for (i, key) in path.iter().enumerate() {
            match at.and_then(|t| t.get(*key)) {
                Some(_) if i + 1 == path.len() => {
                    bail!("{field} sets {}, which zrehearse owns", path.join("."))
                }
                Some(toml::Value::Table(t)) => at = Some(t),
                _ => break,
            }
        }
    }
    Ok(())
}

fn reject_owned_env(field: &str, env: &BTreeMap<String, String>, suffixes: &[&str]) -> Result<()> {
    if let Some(name) = env.keys().find(|k| suffixes.iter().any(|s| k.ends_with(s))) {
        bail!("{field} sets {name}, which zrehearse owns");
    }
    Ok(())
}

/// Merges `extra` into `base`. Tables merge key by key, other values replace.
pub fn merge(base: &mut toml::Table, extra: &toml::Table) {
    for (key, value) in extra {
        match (base.get_mut(key), value) {
            (Some(toml::Value::Table(b)), toml::Value::Table(e)) => merge(b, e),
            _ => {
                base.insert(key.clone(), value.clone());
            }
        }
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
        let p = with_upgrade("name = \"NU6.3\"\nprevious = \"NU6.2\"\nheight = 110\n").unwrap();
        assert_eq!(
            p.zebrad_toml(),
            "[network]\nnetwork = \"Regtest\"\n\n[network.testnet_parameters.activation_heights]\n\
             \"NU6.2\" = 2\n\"NU6.3\" = 110\n"
        );
    }

    #[test]
    fn zebrad_toml_escapes_names() {
        let p = with_upgrade("name = 'a\"b'\nprevious = \"NU6.2\"\n").unwrap();
        let cfg: toml::Table = toml::from_str(&p.zebrad_toml()).unwrap();
        let heights = &cfg["network"]["testnet_parameters"]["activation_heights"];
        assert_eq!(heights["a\"b"].as_integer(), Some(110));
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
        assert!(
            err("name = \"NU6.3\"\nprevious = \"NU6.2\"\nheight = 3\nblocks_after = 97\n")
                .contains("is 100, but it must be above 100")
        );
        assert!(
            with_upgrade("name = \"NU6.3\"\nprevious = \"NU6.2\"\nheight = 3\nblocks_after = 98\n")
                .is_ok()
        );
    }

    #[test]
    fn rejects_duplicate_and_unsafe_project_names() {
        let upgrade = "name = \"NU6.3\"\nprevious = \"NU6.2\"\n";
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
    fn reads_the_light_server() {
        let p = plan(&format!(
            "name = \"t\"\n{NODE}[upgrade]\nname = \"NU7\"\nprevious = \"NU6.3\"\n\
             [light_server]\nkind = \"zaino\"\nimage = \"zingodevops/zaino:0.10.1-no-tls\"\n"
        ))
        .unwrap();
        let ls = p.light_server.unwrap();
        assert!(matches!(ls.kind, LightServerKind::Zaino));
        assert_eq!(ls.ready_timeout_secs, 120);
        let bad = plan(&format!(
            "name = \"t\"\n{NODE}[upgrade]\nname = \"NU7\"\nprevious = \"NU6.3\"\n\
             [light_server]\nkind = \"lwd\"\nimage = \"x\"\n"
        ));
        assert!(bad.unwrap_err().to_string().contains("unknown variant"));
    }

    #[test]
    fn node_config_merges_into_the_generated_config() {
        let p = plan(&format!(
            "name = \"t\"\n{NODE}[node.config.network.testnet_parameters]\n\
             lockbox_disbursements = [{{ address = \"t2x\", amount = 0 }}]\n\
             [node.config.mempool]\ndebug_enable_at_height = 0\n\
             [upgrade]\nname = \"NU6.3\"\nprevious = \"NU6.2\"\n"
        ))
        .unwrap();
        let cfg: toml::Table = toml::from_str(&p.zebrad_toml()).unwrap();
        let tp = &cfg["network"]["testnet_parameters"];
        assert_eq!(tp["activation_heights"]["NU6.3"].as_integer(), Some(110));
        assert_eq!(
            tp["lockbox_disbursements"][0]["address"].as_str(),
            Some("t2x")
        );
        assert_eq!(
            cfg["mempool"]["debug_enable_at_height"].as_integer(),
            Some(0)
        );
        assert_eq!(cfg["network"]["network"].as_str(), Some("Regtest"));
    }

    #[test]
    fn rejects_keys_that_zrehearse_owns() {
        let up = "[upgrade]\nname = \"NU6.3\"\nprevious = \"NU6.2\"\n";
        let err = |extra: &str| {
            plan(&format!("name = \"t\"\n{NODE}{extra}{up}"))
                .unwrap_err()
                .to_string()
        };
        assert_eq!(
            err("[node.config.network.testnet_parameters.activation_heights]\nNU7 = 5\n"),
            "node.config sets network.testnet_parameters.activation_heights, which zrehearse owns"
        );
        assert!(
            err("[node.config.mining]\nminer_address = \"x\"\n").contains("mining.miner_address")
        );
        assert!(
            err("[node.env]\nZAKURA_RPC__LISTEN_ADDR = \"x\"\n")
                .contains("ZAKURA_RPC__LISTEN_ADDR")
        );
        let lwd = "[light_server]\nkind = \"lightwalletd\"\nimage = \"x\"\n";
        assert!(err(&format!("{lwd}args = [\"--rpchost=y\"]\n")).contains("--rpchost=y"));
        assert!(err(&format!("{lwd}[light_server.config]\na = 1\n")).contains("only for zaino"));
        let ok = format!("{lwd}args = [\"--log-level\", \"7\"]\n[light_server.env]\nA = \"1\"\n");
        assert!(plan(&format!("name = \"t\"\n{NODE}{ok}{up}")).is_ok());
    }

    #[test]
    fn defaults_fill_in() {
        let p = with_upgrade("name = \"NU6.3\"\nprevious = \"NU6.2\"\n").unwrap();
        assert_eq!(p.upgrade.blocks_after, 10);
        assert_eq!(p.upgrade.height, 110);
        assert!(p.projects.is_empty());
    }
}
