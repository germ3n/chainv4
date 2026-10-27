//! chainv4 faucet: asks a node's faucet for a drip, doing the proof of work it wants on this
//! machine's CPU, and waits for the node to take it.

use {
    super::CliResult,
    super::send::{TlsArgs, agent, fetch, ok},
    crate::faucet::{enough_work, faucet_hash, faucet_window},
    crate::structs::address::Address,
    crate::structs::hash::{Hash, HashType},
    crate::structs::hex::from_hex,
    serde_json::json,
};

#[derive(clap::Args, Debug)]
pub struct FaucetArgs {
    /// The node's HTTP API: http://, or https:// behind TLS.
    #[arg(long, default_value = "http://127.0.0.1:8080")]
    pub node: String,
    #[command(flatten)]
    pub tls: TlsArgs,
    /// Whom to pay: an address, such as ed25519:0x…, an Ethereum 0x… or a bc1q…, or a name, such as alice.dev.
    #[arg(long)]
    pub to: String,
}

pub fn faucet(args: FaucetArgs) -> CliResult<()> {
    let agent = agent(&args.tls)?;
    let node = args.node.trim_end_matches('/');
    let status = ok(agent.get(format!("{node}/status")).call())?;
    let faucet = &status["faucet"];

    if faucet.is_null() {
        return Err(format!("{node} runs no faucet").into());
    }

    let to = recipient(&agent, node, &args.to)?;
    let hash: HashType = status["hash"].as_str().and_then(|name| name.parse().ok()).ok_or("the node sent no hash")?;
    let genesis = status["genesis"].as_str().and_then(from_hex).and_then(|bytes| Hash::from_bytes(hash, &bytes)).ok_or("the node sent no genesis id that reads")?;
    let bits = u32::try_from(faucet["work_bits"].as_u64().ok_or("the node sent no work_bits")?)?;
    let window = faucet_window(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_millis() as u64);
    let symbol = status["coin"]["symbol"].as_str().unwrap_or("coins").to_string();
    let decimals = u8::try_from(status["coin"]["decimals"].as_u64().unwrap_or(0))?;

    println!("working out the faucet's proof of work: {bits} zero bits, about {} hashes", 1u64 << bits.min(63));
    let started = std::time::Instant::now();
    let nonce = (0..).find(|nonce| enough_work(&faucet_hash(&genesis, &to, window, *nonce), bits)).ok_or("no nonce works")?;
    println!("found in {:.1}s", started.elapsed().as_secs_f64());

    let body = json!({ "to": to.to_string(), "window": window, "nonce": nonce });
    let answer = match fetch(agent.post(format!("{node}/faucet")).header("content-type", "application/json").send(body.to_string()))? {
        (200..=299, answer) => answer,
        (status, answer) => return Err(format!("the faucet refused ({status}): {}", answer["error"].as_str().unwrap_or("no reason given")).into()),
    };
    let amount = answer["amount"].as_str().unwrap_or("0");

    println!("{} {symbol} on its way to {to}: {}", coins(amount, decimals), answer["id"].as_str().unwrap_or("?"));

    Ok(())
}

/// Whom --to names: an address, an Ethereum one as Ethereum writes it, or a name the node resolves.
fn recipient(agent: &ureq::Agent, node: &str, text: &str) -> CliResult<Address> {
    let text = text.trim();

    if text.contains(':') || text.to_ascii_lowercase().starts_with("bc1") {
        return Ok(text.parse()?);
    }

    if text.len() == 42 && text.starts_with("0x") {
        return Ok(format!("secp256k1:{text}").parse()?);
    }

    resolve(agent, node, text)
}

#[cfg(feature = "names")]
fn resolve(agent: &ureq::Agent, node: &str, name: &str) -> CliResult<Address> {
    super::send::resolve(agent, node, name)
}

#[cfg(not(feature = "names"))]
fn resolve(_: &ureq::Agent, _: &str, name: &str) -> CliResult<Address> {
    Err(format!("{name} is not an address, and this build has no names").into())
}

/// An amount in units, as whole coins at the coin's decimals: 10.5.
fn coins(units: &str, decimals: u8) -> String {
    let decimals = usize::from(decimals);
    let padded = format!("{units:0>width$}", width = decimals + 1);
    let (whole, fraction) = padded.split_at(padded.len() - decimals);
    let fraction = fraction.trim_end_matches('0');

    match fraction.is_empty() {
        true => whole.to_string(),
        false => format!("{whole}.{fraction}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts_read_in_whole_coins() {
        assert_eq!(coins("10500000000000000000", 18), "10.5");
        assert_eq!(coins("5", 2), "0.05");
        assert_eq!(coins("700", 2), "7");
        assert_eq!(coins("0", 18), "0");
    }
}
