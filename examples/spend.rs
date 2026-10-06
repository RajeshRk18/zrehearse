//! A sample project for zrehearse. It spends block rewards of the funded key
//! across the upgrade boundary with its own ZIP 244 signer, and expects this
//! result from the node.
//!
//! - A spend signed for `ZREHEARSE_BRANCH_ID` is accepted.
//! - A spend signed for `ZREHEARSE_PREVIOUS_BRANCH_ID` is rejected.
//! - A spend with the new branch ID in its header but a sighash for the
//!   previous branch is rejected. This is the bug of a signer that has the
//!   old branch ID built in.
//!
//! Run it as a plan project with `cargo run --quiet --example spend`.

use anyhow::{Context, Result, bail, ensure};
use blake2b_simd::Params;
use ripemd::Ripemd160;
use secp256k1::{Message, PublicKey, Secp256k1, SecretKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// Base58Check prefixes that regtest shares with testnet
/// (`zcash_protocol/src/constants/regtest.rs`).
const P2PKH_PREFIX: [u8; 2] = [0x1d, 0x25];
const SECRET_KEY_PREFIX: u8 = 0xef;
/// ZIP 317 charges 5000 zats per logical action, with 2 grace actions. One
/// P2PKH input and one P2PKH output is one logical action, so the fee is
/// 2 * 5000.
const FEE: i64 = 10_000;
const COINBASE_MATURITY: u64 = 100;

struct Utxo {
    /// Txid in RPC (display) byte order.
    txid: [u8; 32],
    index: u32,
    value: i64,
}

fn blake(personal: &[u8; 16], data: &[u8]) -> [u8; 32] {
    let h = Params::new().hash_length(32).personal(personal).hash(data);
    h.as_bytes().try_into().unwrap()
}

fn compact(out: &mut Vec<u8>, n: usize) {
    assert!(n < 0xfd, "compact sizes above 252 are not needed here");
    out.push(n as u8);
}

fn p2pkh_script(pkh: &[u8; 20]) -> Vec<u8> {
    [&[0x76, 0xa9, 0x14][..], pkh, &[0x88, 0xac]].concat()
}

/// Builds and signs a transparent-only v5 transaction (ZIP 225) that spends
/// `utxo` back to the same P2PKH address. `header_branch` goes in the
/// nConsensusBranchId field, and `sig_branch` personalises the ZIP 244 sighash.
/// Returns the raw transaction and its ZIP 244 txid in display order.
fn build_v5(
    sk: &SecretKey,
    utxo: &Utxo,
    header_branch: u32,
    sig_branch: u32,
) -> (Vec<u8>, [u8; 32]) {
    let secp = Secp256k1::signing_only();
    let pubkey = PublicKey::from_secret_key(&secp, sk).serialize();
    let script = p2pkh_script(&Ripemd160::digest(Sha256::digest(pubkey)).into());
    let value = utxo.value - FEE;
    let (lock_time, expiry, sequence) = (0u32, 0u32, u32::MAX);

    let mut prevout = utxo.txid;
    prevout.reverse();
    let prevout = [&prevout[..], &utxo.index.to_le_bytes()].concat();
    let mut script_field = Vec::new();
    compact(&mut script_field, script.len());
    script_field.extend(&script);
    let txout = [&value.to_le_bytes()[..], &script_field].concat();

    let header = |branch: u32| {
        [
            0x8000_0005u32.to_le_bytes(), // v5, fOverwintered
            0x26a7_270au32.to_le_bytes(), // nVersionGroupId
            branch.to_le_bytes(),
            lock_time.to_le_bytes(),
            expiry.to_le_bytes(),
        ]
        .concat()
    };
    // ZIP 244 T.1 header digest, T.2a-c transparent parts, empty T.3 and T.4.
    let header_digest = |branch: u32| blake(b"ZTxIdHeadersHash", &header(branch));
    let prevouts = blake(b"ZTxIdPrevoutHash", &prevout);
    let sequences = blake(b"ZTxIdSequencHash", &sequence.to_le_bytes());
    let outputs = blake(b"ZTxIdOutputsHash", &txout);
    let sapling = blake(b"ZTxIdSaplingHash", &[]);
    let orchard = blake(b"ZTxIdOrchardHash", &[]);
    let root = |branch: u32, transparent: [u8; 32]| {
        let mut personal = *b"ZcashTxHash_\0\0\0\0";
        personal[12..].copy_from_slice(&branch.to_le_bytes());
        let parts = [header_digest(branch), transparent, sapling, orchard];
        blake(&personal, &parts.concat())
    };

    // ZIP 244 S.2 with SIGHASH_ALL.
    let txin = [
        &prevout[..],
        &utxo.value.to_le_bytes(),
        &script_field,
        &sequence.to_le_bytes(),
    ]
    .concat();
    let sig_parts = [
        prevouts,
        blake(b"ZTxTrAmountsHash", &utxo.value.to_le_bytes()),
        blake(b"ZTxTrScriptsHash", &script_field),
        sequences,
        outputs,
        blake(b"Zcash___TxInHash", &txin),
    ];
    let transparent_sig = blake(
        b"ZTxIdTranspaHash",
        &[&[0x01][..], &sig_parts.concat()].concat(),
    );
    let sighash = root(sig_branch, transparent_sig);
    let sig = secp.sign_ecdsa(&Message::from_digest(sighash), sk);
    let sig = [&sig.serialize_der()[..], &[0x01]].concat();
    let script_sig = [&[sig.len() as u8][..], &sig, &[33], &pubkey].concat();

    let mut tx = header(header_branch);
    compact(&mut tx, 1);
    tx.extend(&prevout);
    compact(&mut tx, script_sig.len());
    tx.extend(&script_sig);
    tx.extend(sequence.to_le_bytes());
    compact(&mut tx, 1);
    tx.extend(&txout);
    tx.extend([0, 0, 0]); // no Sapling spends, Sapling outputs, Orchard actions

    let transparent_txid = blake(
        b"ZTxIdTranspaHash",
        &[prevouts, sequences, outputs].concat(),
    );
    let mut txid = root(header_branch, transparent_txid);
    txid.reverse();
    (tx, txid)
}

fn rpc(agent: &ureq::Agent, url: &str, method: &str, params: Value) -> Result<Value> {
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    let reply: Value = agent
        .post(url)
        .send_json(&body)
        .with_context(|| format!("calling {method}"))?
        .body_mut()
        .read_json()
        .with_context(|| format!("reading {method} reply"))?;
    if let Some(err) = reply.get("error").filter(|e| !e.is_null()) {
        bail!("{err}");
    }
    reply.get("result").cloned().context("reply has no result")
}

fn env(name: &str) -> Result<String> {
    std::env::var(name).with_context(|| format!("{name} is not set"))
}

fn branch(name: &str) -> Result<u32> {
    let v = env(name)?;
    u32::from_str_radix(&v, 16).with_context(|| format!("{name}={v} is not 8 hex chars"))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex32(s: &str) -> Result<[u8; 32]> {
    ensure!(s.len() == 64, "{s} is not 32 bytes of hex");
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[2 * i..2 * i + 2], 16)?;
    }
    Ok(out)
}

fn main() -> Result<()> {
    let url = env("ZREHEARSE_RPC_URL")?;
    let address = env("ZREHEARSE_FUNDED_ADDRESS")?;
    let (current, previous) = (
        branch("ZREHEARSE_BRANCH_ID")?,
        branch("ZREHEARSE_PREVIOUS_BRANCH_ID")?,
    );

    let wif = bs58::decode(env("ZREHEARSE_FUNDED_KEY")?)
        .with_check(None)
        .into_vec()?;
    ensure!(
        wif.len() == 34 && wif[0] == SECRET_KEY_PREFIX && wif[33] == 0x01,
        "ZREHEARSE_FUNDED_KEY is not a compressed testnet WIF key"
    );
    let sk = SecretKey::from_slice(&wif[1..33])?;
    let pubkey = PublicKey::from_secret_key(&Secp256k1::signing_only(), &sk).serialize();
    let pkh = Ripemd160::digest(Sha256::digest(pubkey));
    let decoded = bs58::decode(&address).with_check(None).into_vec()?;
    ensure!(
        decoded == [&P2PKH_PREFIX[..], &pkh].concat(),
        "ZREHEARSE_FUNDED_ADDRESS {address} is not the P2PKH address of ZREHEARSE_FUNDED_KEY"
    );

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .into();
    let tip = rpc(&agent, &url, "getblockcount", json!([]))?
        .as_u64()
        .context("getblockcount is not a number")?;
    let utxos = rpc(
        &agent,
        &url,
        "getaddressutxos",
        json!([{"addresses": [address]}]),
    )?;
    let mut mature: Vec<(u64, Utxo)> = Vec::new();
    for u in utxos
        .as_array()
        .context("getaddressutxos is not an array")?
    {
        let height = u["height"].as_u64().context("utxo has no height")?;
        // The spend goes in block tip + 1.
        if height + COINBASE_MATURITY <= tip + 1 {
            mature.push((
                height,
                Utxo {
                    txid: unhex32(u["txid"].as_str().context("utxo has no txid")?)?,
                    index: u["outputIndex"]
                        .as_u64()
                        .context("utxo has no outputIndex")? as u32,
                    value: u["satoshis"].as_i64().context("utxo has no satoshis")?,
                },
            ));
        }
    }
    mature.sort_by_key(|(h, _)| *h);
    ensure!(
        mature.len() >= 3,
        "need 3 mature UTXOs at tip {tip}, found {}",
        mature.len()
    );

    let cases = [
        ("current branch", current, current, true),
        ("previous branch", previous, previous, false),
        ("current header, previous sighash", current, previous, false),
    ];
    let mut ok = true;
    for ((name, header, sig, expect_accept), (height, utxo)) in cases.iter().zip(&mature) {
        let (tx, txid) = build_v5(&sk, utxo, *header, *sig);
        println!(
            "{name}: spend {}:{} (height {height}, {} zats, fee {FEE}) header={header:08x} sighash={sig:08x}",
            hex(&utxo.txid),
            utxo.index,
            utxo.value
        );
        match rpc(&agent, &url, "sendrawtransaction", json!([hex(&tx)])) {
            Ok(id) => {
                println!("  accepted txid {id} (computed {})", hex(&txid));
                ok &= *expect_accept && id.as_str() == Some(&hex(&txid));
            }
            Err(e) => {
                println!("  rejected: {e}");
                ok &= !*expect_accept;
            }
        }
    }
    ensure!(ok, "an expectation failed");
    println!("all expectations hold");
    Ok(())
}
