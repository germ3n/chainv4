//! `chainv4 calibrate`: what fuel should cost, measured on this machine. Lua's prices are a table
//! (`LuaCosts`, genesis's `fees.lua_costs`) and the host's another (`HostCosts`, `fees.costs`), both
//! hand-picked to start with. This runs a probe for each priced thing a contract can do many times,
//! in a deployed Lua contract on an in-memory chain, timing it against a base that does everything
//! but that thing, and sets what it took beside what it is charged.
//!
//! One fuel is anchored to the opcode probes together: what they took over what they are charged,
//! so the table keeps today's price level overall and only what each costs against the others
//! moves; `--ns-per-fuel` sets it instead. The cheapest opcodes take hundredths of a nanosecond, too
//! little to time alone, as the processor runs them alongside the loop. An opcode's suggested cost is its
//! time in those, a library rate's what makes its charge match its time, and a host operation's its
//! time past a cheap call into the host. The suggestions are written as a fees object's
//! `lua_costs` and `costs`, for a genesis file or a change of fees, and a line marks each thing
//! charged less than half of what it costs, the way to stall every node with cheap work.
//!
//! WebAssembly's instructions and the EVM's opcodes are priced by wasmi and revm, and not probed
//! here yet.

use {
    super::CliResult,
    crate::db::{MemoryDb, State},
    crate::params::vm::{HostCosts, LuaCosts},
    crate::registry::contracts::Contracts,
    crate::structs::address::Address,
    crate::structs::code::{Code, Engine},
    crate::structs::hash::{Hash, HashType},
    crate::structs::tx::TxCallArgumentType,
    crate::structs::u256::u256,
    crate::vm::{BlockAt, CallContext, Limits, Vm},
    serde_json::{Value, json},
    std::path::PathBuf,
    std::sync::Arc,
    std::time::Instant,
};

#[derive(clap::Args)]
pub struct CalibrateArgs {
    /// How many times each probe's body runs in one call.
    #[arg(long, default_value_t = 200_000)]
    pub iterations: u64,
    /// How many calls each probe is timed over: the fastest counts, as the least disturbed.
    #[arg(long, default_value_t = 7)]
    pub rounds: u32,
    /// The nanoseconds one fuel stands for, rather than what the opcode probes take over what they
    /// are charged.
    #[arg(long)]
    pub ns_per_fuel: Option<f64>,
    /// Write the measurements and the suggested costs, as JSON, to this file too.
    #[arg(long)]
    pub json: Option<PathBuf>,
}

/// What a probe prices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Priced {
    /// A Lua opcode's cost, by its slot's name.
    Opcode(&'static str),
    /// A Lua library rate: bytes or elements a fuel pays for, the more the cheaper.
    Rate(&'static str),
    /// A Lua library step's cost, such as a sort comparison.
    Step(&'static str),
    /// A host operation's fixed cost, by its HostCosts field.
    Host(&'static str),
}

/// One thing timed: the loop's body against a base body, each run `iterations` times after the
/// setup, in a contract of its own. `ops` is how many of the priced thing one body does more than
/// the base, and `with` the opcodes it runs besides, each once, whose own costs come off.
struct Probe {
    priced: Priced,
    setup: &'static str,
    body: &'static str,
    base: &'static str,
    ops: f64,
    with: &'static [&'static str],
}

/// Values the compiler cannot fold, as they come from the call's argument.
const LOCALS: &str = "n = n:tonumber()\nlocal z = n - n\nlocal a, c, two, b = z + 12, z + 3, z + 2, z\nlocal x, y, s = 'ab' .. z, 'cd' .. z, ''\nlocal t = { x = 1, z + 1 }\nlocal k = z + 1\nlocal f0 = function() end\n";

const fn op(name: &'static str, body: &'static str) -> Probe {
    Probe { priced: Priced::Opcode(name), setup: "", body, base: "", ops: 1.0, with: &[] }
}

const fn op_with(name: &'static str, body: &'static str, with: &'static [&'static str]) -> Probe {
    Probe { priced: Priced::Opcode(name), setup: "", body, base: "", ops: 1.0, with }
}

/// The probes, those another subtracts measured before it.
const PROBES: &[Probe] = &[
    op("MOVE", "b = a"),
    op("LOADN", "b = 5"),
    op("LOADK", "b = 100000"),
    op("NOT", "b = not a"),
    op("MINUS", "b = -a"),
    op("ADD", "b = a + c"),
    op("SUB", "b = a - c"),
    op("MUL", "b = a * c"),
    op("DIV", "b = a / c"),
    op("MOD", "b = a % c"),
    op("IDIV", "b = a // c"),
    op("POW", "b = c ^ two"),
    op("ADDK", "b = a + 1"),
    op("MULK", "b = a * 2"),
    op("CONCAT", "s = x .. y"),
    op("LENGTH", "b = #t"),
    op("GETTABLE", "b = t[k]"),
    op("SETTABLE", "t[k] = a"),
    op("GETTABLEKS", "b = t.x"),
    op("SETTABLEKS", "t.x = a"),
    op("GETTABLEN", "b = t[1]"),
    op("SETTABLEN", "t[1] = a"),
    op("GETIMPORT", "b = type"),
    op_with("JUMPIFNOTEQ", "if a == c then b = a end", &[]),
    op_with("JUMPIFNOTLT", "if a < c then b = a end", &[]),
    op("NEWTABLE", "local u = {}"),
    op("DUPTABLE", "local u = { x = 1, y = 2 }"),
    op("DUPCLOSURE", "local g = function() end"),
    op("NEWCLOSURE", "local g = function() return a end"),
    op_with("CALL", "f0()", &["RETURN"]),
    op("FASTCALL2", "b = bit32.band(a, c)"),
    op_with("NAMECALL", "b = x:len()", &["CALL"]),
    // Library rates: the same call at two sizes, so only the work that grows with its size is left.
    Probe { priced: Priced::Rate("bytes_per_fuel"), setup: "", body: "s = string.rep(x, 4096)", base: "s = string.rep(x, 64)", ops: (4096.0 - 64.0) * 3.0, with: &[] },
    Probe {
        priced: Priced::Rate("elements_per_fuel"),
        setup: "local big, small = {}, {}\nfor i = 1, 1024 do big[i] = 'e' end\nfor i = 1, 16 do small[i] = 'e' end\n",
        body: "s = table.concat(big)",
        base: "s = table.concat(small)",
        ops: 1024.0 - 16.0,
        with: &[],
    },
    // A sort's comparisons: sorting a copy against making the copy, 256 in reverse.
    Probe {
        priced: Priced::Step("sort_comparison"),
        setup: "local src = {}\nfor i = 1, 256 do src[i] = 257 - i end\n",
        body: "local u = table.clone(src) table.sort(u)",
        base: "local u = table.clone(src)",
        ops: 256.0 * 9.0,
        with: &[],
    },
    // A pattern that backtracks: each try at each position is a step.
    Probe {
        priced: Priced::Step("match_step"),
        setup: "local hay = string.rep('a', 256)\n",
        body: "b = string.find(hay, 'a-b')",
        base: "b = string.find(hay, 'b', 1, true)",
        ops: 256.0 * 257.0 / 2.0,
        with: &[],
    },
    // Host operations, past a cheap call into the host: tostring is the chain's own.
    Probe { priced: Priced::Host("storage_read"), setup: "storage.set('k', 'v')\n", body: "s = storage.get('k')", base: "s = tostring(a)", ops: 1.0, with: &[] },
    Probe { priced: Priced::Host("storage_write"), setup: "", body: "storage.set('k', x)", base: "s = tostring(a)", ops: 1.0, with: &[] },
    Probe { priced: Priced::Host("hash"), setup: "", body: "s = hash.sha256(x)", base: "s = tostring(a)", ops: 1.0, with: &[] },
];

/// What a probe measured.
#[derive(Clone, Debug)]
struct Measured {
    priced: Priced,
    /// Nanoseconds one of the priced thing took.
    ns: f64,
    /// Fuel it was charged.
    fuel: f64,
}

/// A chain in memory to deploy and call the probes on.
struct Bench {
    state: Arc<State>,
    vm: Arc<Vm>,
    nonce: u64,
}

const DEPLOYER: Address = Address::Ed25519([7; 32]);
const LIMITS: Limits = Limits { fuel: u64::MAX / 4, memory: 256 * 1024 * 1024 };

impl Bench {
    fn new() -> CliResult<Bench> {
        Ok(Bench {
            state: Arc::new(State::new(Arc::new(MemoryDb::default()))),
            vm: Arc::new(Vm::new(&[Engine::Lua], HashType::default_chain()).map_err(|engine| format!("this build has no {engine:?} engine"))?),
            nonce: 0,
        })
    }

    fn deploy(&mut self, setup: &str, body: &str) -> CliResult<Address> {
        self.nonce += 1;
        let source = format!("local M = {{}}\nfunction M.run(n)\n{LOCALS}{setup}for i = 1, n do\n{body}\nend\nend\nreturn M\n");
        let standards = crate::registry::Standards { tokens: Vec::<String>::new().into(), collections: None };

        Contracts::new(&self.state)
            .deploy(&self.vm, &DEPLOYER, self.nonce, &Code::Lua(source), LIMITS, &standards, BlockAt::at(0))
            .map(|(address, _)| address)
            .map_err(|failed| format!("a probe did not deploy: {}", failed.error).into())
    }

    /// The fastest of the rounds, and the fuel it burned.
    fn time(&self, contract: Address, iterations: u64, rounds: u32) -> CliResult<(f64, f64)> {
        let context = CallContext { sender: DEPLOYER, contract, value: u256::ZERO, block: BlockAt::at(0) };
        let func = Hash::Xxh3_64(crate::vm::selector("run"));
        let args = [TxCallArgumentType::I64(iterations as i64)];
        let mut best = f64::MAX;
        let mut fuel = 0;

        for _ in 0..rounds.max(1) {
            let start = Instant::now();
            let outcome = Contracts::new(&self.state).call(&self.vm, &context, &func, &args, LIMITS)?;
            let took = start.elapsed().as_nanos() as f64;

            if let Err(err) = outcome.result {
                return Err(format!("a probe failed: {err}").into());
            }

            best = best.min(took);
            fuel = outcome.fuel_used;
        }

        Ok((best, fuel as f64))
    }
}

pub fn calibrate(args: CalibrateArgs) -> CliResult<()> {
    let mut bench = Bench::new()?;
    let mut measured: Vec<Measured> = Vec::new();

    for probe in PROBES {
        let body = bench.deploy(probe.setup, probe.body)?;
        let base = bench.deploy(probe.setup, probe.base)?;
        let ((body_ns, body_fuel), (base_ns, base_fuel)) = (bench.time(body, args.iterations, args.rounds)?, bench.time(base, args.iterations, args.rounds)?);
        let count = args.iterations as f64 * probe.ops;
        let mut ns = ((body_ns - base_ns) / count).max(0.0);
        let mut fuel = (body_fuel - base_fuel) / count;

        // What else the body runs comes off, by what it was measured to cost and is charged.
        for other in probe.with {
            if let Some(other) = measured.iter().find(|done| done.priced == Priced::Opcode(other)) {
                ns = (ns - other.ns).max(0.0);
                fuel -= other.fuel;
            } else if let Some(slot) = LuaCosts::slot(other) {
                fuel -= LuaCosts::DEFAULT.slots[slot] as f64;
            }
        }

        measured.push(Measured { priced: probe.priced, ns, fuel });
    }

    let ns_per_fuel = args.ns_per_fuel.unwrap_or_else(|| anchor(&measured));
    let (lua, host) = suggest(&measured, ns_per_fuel);
    print(&measured, ns_per_fuel, &lua, &host);

    if let Some(path) = args.json {
        std::fs::write(&path, serde_json::to_string_pretty(&report(&measured, ns_per_fuel, &lua, &host))?)?;
        println!("wrote {}", path.display());
    }

    Ok(())
}

/// The nanoseconds a fuel stands for: what the opcode probes took, together, over the fuel they are
/// charged together.
fn anchor(measured: &[Measured]) -> f64 {
    let opcodes = measured.iter().filter(|probe| matches!(probe.priced, Priced::Opcode(_)));
    let (ns, fuel) = opcodes.fold((0.0, 0.0), |(ns, fuel), probe| (ns + probe.ns, fuel + probe.fuel.max(0.0)));

    if fuel > 0.0 && ns > 0.0 { ns / fuel } else { 1.0 }
}

/// The costs the measurements suggest: Lua's table and the host's, each from today's.
fn suggest(measured: &[Measured], ns_per_fuel: f64) -> (LuaCosts, HostCosts) {
    let mut lua = LuaCosts::DEFAULT;
    let mut host = HostCosts::DEFAULT;
    let fair = |ns: f64| (ns / ns_per_fuel).round().max(1.0) as u64;

    for probe in measured {
        match probe.priced {
            Priced::Opcode(name) | Priced::Step(name) => lua.slots[LuaCosts::slot(name).expect("a probe's slot")] = fair(probe.ns),
            Priced::Rate(name) => {
                // As many bytes or elements a fuel as make the charge for them what they took.
                let slot = LuaCosts::slot(name).expect("a probe's slot");
                let per_fuel = (ns_per_fuel / probe.ns.max(f64::MIN_POSITIVE)).round().max(1.0);
                lua.slots[slot] = per_fuel.min(1e9) as u64;
            }
            Priced::Host(name) => set_host(&mut host, name, fair(probe.ns)),
        }
    }

    (lua, host)
}

fn set_host(costs: &mut HostCosts, name: &str, cost: u64) {
    match name {
        "storage_read" => costs.storage_read = cost,
        "storage_write" => costs.storage_write = cost,
        "hash" => costs.hash = cost,
        _ => unreachable!("a host probe without a field: {name}"),
    }
}

fn host_cost(costs: &HostCosts, name: &str) -> u64 {
    match name {
        "storage_read" => costs.storage_read,
        "storage_write" => costs.storage_write,
        "hash" => costs.hash,
        _ => unreachable!("a host probe without a field: {name}"),
    }
}

/// What a probe is charged today and what it is suggested at, for one of it.
fn charged_and_suggested(probe: &Measured, lua: &LuaCosts, host: &HostCosts) -> (String, String, String, bool) {
    match probe.priced {
        Priced::Opcode(name) | Priced::Step(name) => {
            let slot = LuaCosts::slot(name).expect("a probe's slot");
            let now = LuaCosts::DEFAULT.slots[slot];

            (name.to_string(), now.to_string(), lua.slots[slot].to_string(), (now as f64) < lua.slots[slot] as f64 / 2.0)
        }
        Priced::Rate(name) => {
            let slot = LuaCosts::slot(name).expect("a probe's slot");
            let now = LuaCosts::DEFAULT.slots[slot];

            // More per fuel is cheaper: charged too little when today's is more than twice it.
            (name.to_string(), now.to_string(), lua.slots[slot].to_string(), now > lua.slots[slot] * 2)
        }
        Priced::Host(name) => {
            let now = host_cost(&HostCosts::DEFAULT, name);

            (format!("host {name}"), now.to_string(), host_cost(host, name).to_string(), (now as f64) < host_cost(host, name) as f64 / 2.0)
        }
    }
}

fn print(measured: &[Measured], ns_per_fuel: f64, lua: &LuaCosts, host: &HostCosts) {
    println!("one fuel stands for {ns_per_fuel:.2} ns here");
    println!("{:<22} {:>10} {:>10} {:>10} {:>10}", "", "ns", "fuel now", "charged", "suggested");

    for probe in measured {
        let (name, now, suggested, cheap) = charged_and_suggested(probe, lua, host);
        let flag = if cheap { "  charged under half of what it costs" } else { "" };
        println!("{name:<22} {:>10.2} {now:>10} {:>10.2} {suggested:>10}{flag}", probe.ns, probe.fuel);
    }

    println!("WebAssembly's and the EVM's own instructions are priced by wasmi and revm, and not probed.");
}

fn report(measured: &[Measured], ns_per_fuel: f64, lua: &LuaCosts, host: &HostCosts) -> Value {
    let probes: Vec<Value> = measured
        .iter()
        .map(|probe| {
            let (name, now, suggested, cheap) = charged_and_suggested(probe, lua, host);

            json!({ "name": name, "ns": probe.ns, "fuel_now": now, "charged": probe.fuel, "suggested": suggested, "charged_too_little": cheap })
        })
        .collect();

    json!({
        "ns_per_fuel": ns_per_fuel,
        "probes": probes,
        "fees": { "lua_costs": crate::params::json::lua_costs_json(lua), "costs": crate::params::json::costs_json(host) },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_probe_deploys_runs_and_names_a_slot_or_a_host_cost() {
        let mut bench = Bench::new().unwrap();

        for probe in PROBES {
            match probe.priced {
                Priced::Opcode(name) | Priced::Rate(name) | Priced::Step(name) => assert!(LuaCosts::slot(name).is_some(), "{name}"),
                Priced::Host(name) => drop(host_cost(&HostCosts::DEFAULT, name)),
            }

            for body in [probe.body, probe.base] {
                let contract = bench.deploy(probe.setup, body).unwrap_or_else(|err| panic!("{body}: {err}"));
                bench.time(contract, 3, 1).unwrap_or_else(|err| panic!("{body}: {err}"));
            }
        }
    }

    #[test]
    fn a_body_charged_more_than_its_base_shows_what_it_was_charged() {
        let mut bench = Bench::new().unwrap();
        let (adds, base) = (bench.deploy("", "b = a + c").unwrap(), bench.deploy("", "").unwrap());
        let ((_, with), (_, without)) = (bench.time(adds, 100, 1).unwrap(), bench.time(base, 100, 1).unwrap());

        assert_eq!((with - without) / 100.0, LuaCosts::DEFAULT.slots[LuaCosts::slot("ADD").unwrap()] as f64);
    }

    #[test]
    fn one_fuel_is_what_the_opcodes_took_over_what_they_are_charged() {
        let measured = [
            Measured { priced: Priced::Opcode("MOVE"), ns: 2.0, fuel: 1.0 },
            Measured { priced: Priced::Opcode("ADD"), ns: 10.0, fuel: 2.0 },
            Measured { priced: Priced::Host("hash"), ns: 400.0, fuel: 20.0 },
        ];

        assert_eq!(anchor(&measured), 4.0, "12 ns over 3 fuel: the host's is not an opcode");
        assert_eq!(anchor(&[]), 1.0);
    }

    #[test]
    fn suggestions_scale_by_the_anchor_and_rates_go_the_other_way() {
        let measured = [
            Measured { priced: Priced::Opcode("ADD"), ns: 10.0, fuel: 2.0 },
            Measured { priced: Priced::Rate("bytes_per_fuel"), ns: 0.5, fuel: 1.0 / 64.0 },
            Measured { priced: Priced::Host("hash"), ns: 400.0, fuel: 20.0 },
        ];
        let (lua, host) = suggest(&measured, 2.0);

        assert_eq!(lua.slots[LuaCosts::slot("ADD").unwrap()], 5);
        assert_eq!(lua.slots[LuaCosts::slot("bytes_per_fuel").unwrap()], 4, "half a ns a byte: four bytes a fuel");
        assert_eq!(host.hash, 200);
        assert!(charged_and_suggested(&measured[0], &lua, &host).3, "ADD at 2 against 5 is under half");
        assert!(charged_and_suggested(&measured[1], &lua, &host).3, "64 bytes a fuel against 4 is far too cheap");
        assert!(charged_and_suggested(&measured[2], &lua, &host).3);
    }
}
