//! What the browser's miner and its Web Workers share, apart from any browser: the chunks of
//! nonces a worker hashes and what it finds in them, the range of nonces a connection's work
//! comes with, the job they are for, the GPU's prefix and target, hashrates, and what each message
//! from the node does to the status. The Web Worker's binary builds it natively too, so its tests
//! run there.

use {
    chainv4::consensus::work_hash,
    chainv4::mining::protocol::{self, Hello, Joining, Opener, Sealer},
    chainv4::mining::status::{Mode, Work, WorkerStatus},
    chainv4::structs::hash::{Hash, HashType},
    chainv4::structs::hex::{from_hex, to_hex},
    chainv4::structs::u256::u256,
    serde_json::{Value, json},
    std::time::Duration,
    web_time::Instant,
};

/// What a Web Worker says once it is ready for chunks: messages posted before are lost.
pub const READY: &str = "ready";

/// How many nonces a connection's work starts it on, when the work does not say: the node gives
/// each connection a range this wide, so no two try the same nonces.
pub const SPAN: u64 = 1 << 48;

/// How long a Web Worker's chunk should take: short, so new work reaches it soon.
const CHUNK_MS: f64 = 100.0;
/// A Web Worker's first chunk, before its speed is known.
pub const FIRST_CHUNK: u64 = 1 << 14;
/// The fewest and most nonces in a chunk.
const LEAST_CHUNK: u64 = 1 << 10;
const MOST_CHUNK: u64 = 1 << 24;
/// The most nonces one chunk reports at a pool's share difficulty: shares come seconds apart.
const MOST_FOUND: usize = 64;

/// How long a hashrate is measured over before it is taken again.
const STRETCH: Duration = Duration::from_secs(3);

/// The most a work hash may be at a difficulty: `u256::MAX / difficulty`, as `meets` holds it.
fn most(difficulty: u128) -> u256 {
    u256::MAX / u256::from(difficulty.max(1))
}

/// A run of nonces for a Web Worker to hash, for one job: every nonce from start, count of them.
#[derive(Clone, Debug, PartialEq)]
pub struct Chunk {
    /// The job's generation, so what comes back for an old job is told apart.
    pub generation: u64,
    /// The chain's work hash.
    pub hash: HashType,
    /// The block id to find a nonce for.
    pub id: Hash,
    /// What a nonce's work hash must meet: a pool's share difficulty, or the block's.
    pub difficulty: u128,
    pub start: u64,
    pub count: u64,
    /// Whether every nonce that meets it is wanted, a pool's shares, rather than the first.
    pub shares: bool,
}

impl Chunk {
    /// The chunk as the message posted to a Web Worker.
    pub fn to_json(&self) -> String {
        json!({
            "generation": self.generation,
            "hash": self.hash.name(),
            "chain": self.id.algorithm().name(),
            "id": to_hex(&self.id.to_bytes()),
            "difficulty": self.difficulty.to_string(),
            "start": self.start,
            "count": self.count,
            "shares": self.shares,
        })
        .to_string()
    }

    /// A chunk from its message; None for one that does not read.
    pub fn read(text: &str) -> Option<Chunk> {
        let message: Value = serde_json::from_str(text).ok()?;
        let chain: HashType = message["chain"].as_str()?.parse().ok()?;

        Some(Chunk {
            generation: message["generation"].as_u64()?,
            hash: message["hash"].as_str()?.parse().ok()?,
            id: Hash::from_bytes(chain, &from_hex(message["id"].as_str()?)?)?,
            difficulty: message["difficulty"].as_str()?.parse().ok()?,
            start: message["start"].as_u64()?,
            count: message["count"].as_u64()?,
            shares: message["shares"].as_bool()?,
        })
    }

    /// Hashes the chunk's nonces, as the node checks them: up to the first that meets the
    /// difficulty, or for shares every one, a few at most.
    pub fn search(&self) -> Searched {
        let most = most(self.difficulty);
        let mut found = Vec::new();
        let mut tried = 0;

        for nonce in (0..self.count).map(|offset| self.start.wrapping_add(offset)) {
            tried += 1;

            if work_hash(self.hash, &self.id, nonce) <= most {
                found.push(nonce);

                if !self.shares || found.len() >= MOST_FOUND {
                    break;
                }
            }
        }

        Searched { generation: self.generation, tried, found }
    }
}

/// What a Web Worker found in a chunk: the nonces it hashed, and those that met the difficulty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Searched {
    pub generation: u64,
    pub tried: u64,
    pub found: Vec<u64>,
}

impl Searched {
    /// What a Web Worker posts back.
    pub fn to_json(&self) -> String {
        json!({ "generation": self.generation, "tried": self.tried, "found": self.found }).to_string()
    }

    /// What a Web Worker posted back; None for anything else.
    pub fn read(text: &str) -> Option<Searched> {
        let message: Value = serde_json::from_str(text).ok()?;

        Some(Searched {
            generation: message["generation"].as_u64()?,
            tried: message["tried"].as_u64()?,
            found: message["found"].as_array()?.iter().map(Value::as_u64).collect::<Option<_>>()?,
        })
    }
}

/// A Web Worker's next chunk, from its last: as many nonces as it hashes in about CHUNK_MS, at
/// most four times more or fewer than the last, so one slow chunk does not swing it.
pub fn next_count(count: u64, took_ms: f64) -> u64 {
    let wanted = (count as f64 * CHUNK_MS / took_ms.max(1.0)) as u64;

    wanted.clamp(count / 4, count.saturating_mul(4)).clamp(LEAST_CHUNK, MOST_CHUNK)
}

/// The nonces a connection's work leaves to try, handed out from the bottom up, so the GPU's
/// batches and the Web Workers' chunks never overlap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Range {
    next: u64,
    end: u64,
}

impl Range {
    /// The span nonces from start.
    pub fn new(start: u64, span: u64) -> Range {
        Range { next: start, end: start.saturating_add(span) }
    }

    /// The next count nonces, or fewer at the range's end, as their first and how many; None once
    /// all were handed out.
    pub fn take(&mut self, count: u64) -> Option<(u64, u64)> {
        let count = count.min(self.end - self.next);

        if count == 0 {
            return None;
        }

        let first = self.next;
        self.next += count;

        Some((first, count))
    }
}

/// The work being mined: the node's job, the nonces left of it, and whether it is done, a block
/// nonce found and no pool's shares wanted.
pub struct Job {
    pub generation: u64,
    /// The node's number for it, which a found message names.
    pub number: u64,
    pub hash: HashType,
    pub id: Hash,
    /// What a nonce must meet: a pool's share difficulty, or the block's.
    pub difficulty: u128,
    pub shares: bool,
    range: Range,
    done: bool,
}

impl Job {
    /// The job a work message hands, its nonces the span from its start.
    pub fn new(work: &Work, span: u64, generation: u64) -> Job {
        Job {
            generation,
            number: work.job,
            hash: work.hash,
            id: work.id,
            difficulty: work.share.unwrap_or(work.difficulty),
            shares: work.share.is_some(),
            range: Range::new(work.start, span),
            done: false,
        }
    }

    /// The next count nonces to try; None once the job is done or its range spent.
    pub fn take(&mut self, count: u64) -> Option<(u64, u64)> {
        if self.done {
            return None;
        }

        self.range.take(count)
    }

    /// A Web Worker's next chunk of this job.
    pub fn chunk(&mut self, count: u64) -> Option<Chunk> {
        let (start, count) = self.take(count)?;

        Some(Chunk { generation: self.generation, hash: self.hash, id: self.id, difficulty: self.difficulty, start, count, shares: self.shares })
    }

    /// The nonces to send of those found for a generation of the job: none for another's, or once
    /// done; the first alone for a block, which ends the job; every one for a pool's shares.
    pub fn found(&mut self, generation: u64, nonces: &[u64]) -> Vec<u64> {
        if generation != self.generation || self.done || nonces.is_empty() {
            return Vec::new();
        }

        if self.shares {
            return nonces.to_vec();
        }

        self.done = true;

        nonces[..1].to_vec()
    }

    /// Whether a nonce meets the job, checked on the CPU, as the node checks it.
    pub fn meets(&self, nonce: u64) -> bool {
        work_hash(self.hash, &self.id, nonce) <= most(self.difficulty)
    }

    /// The work message before its nonce, as the GPU takes it: `"chainv4 work"` and the block's
    /// 32-byte id; None for an id of another length, which the GPU does not hash.
    pub fn prefix(&self) -> Option<[u8; 44]> {
        let id: [u8; 32] = self.id.to_bytes().try_into().ok()?;
        let mut prefix = [0; 44];
        prefix[..12].copy_from_slice(b"chainv4 work");
        prefix[12..].copy_from_slice(&id);

        Some(prefix)
    }

    /// The most a nonce's work hash may be, as the GPU takes it: 32 big-endian bytes.
    pub fn target(&self) -> [u8; 32] {
        most(self.difficulty).to_be_bytes()
    }
}

/// A device's hashrate: the nonces it tried, and the rate over the last stretch, taken again once
/// it is a few seconds old.
pub struct Meter {
    tried: u64,
    /// When the rate was last taken, the count then, and the rate.
    taken: (Instant, u64, f64),
}

impl Meter {
    pub fn new(now: Instant) -> Meter {
        Meter { tried: 0, taken: (now, 0, 0.0) }
    }

    /// Counts nonces tried.
    pub fn add(&mut self, nonces: u64) {
        self.tried += nonces;
    }

    /// Hashes a second over the last stretch.
    pub fn rate(&mut self, now: Instant) -> f64 {
        let elapsed = now.saturating_duration_since(self.taken.0);

        if elapsed >= STRETCH {
            self.taken = (now, self.tried, (self.tried - self.taken.1) as f64 / elapsed.as_secs_f64());
        }

        self.taken.2
    }
}

/// A connection to the mining port, as far as its handshake is: the node greets, the miner says
/// hello, the node proves it knows the token, and every message after is sealed, both ways.
pub enum Session {
    /// Connecting, then waiting for the node's greeting.
    Greeting,
    /// Said hello, and waiting for the node's proof.
    Joining(Joining),
    Open { sealer: Sealer, opener: Opener },
    Closed,
}

/// What a line from the node came to.
#[derive(Debug, PartialEq)]
pub enum Taken {
    /// The greeting: the hello to send back, as it is.
    Hello(String),
    /// The node's proof: past the handshake.
    Joined,
    /// A message, once its MAC checked.
    Message(Value),
    /// A line for a connection already closed.
    Nothing,
}

impl Session {
    /// Takes the node's next line; an error for one that does not read or check, a node of another
    /// protocol, or one without the token, which ends the connection.
    pub fn take(&mut self, line: &str, token: &[u8], protocol: &str, me: &Hello) -> Result<Taken, String> {
        match std::mem::replace(self, Session::Closed) {
            Session::Greeting => {
                let greeting: Value = serde_json::from_str(line).map_err(|_| "a greeting that does not read: not a mining port".to_string())?;
                let (hello, joining) = protocol::hello(&greeting, token, protocol, me)?;
                *self = Session::Joining(joining);

                Ok(Taken::Hello(hello.to_string()))
            }
            Session::Joining(joining) => {
                let answer: Value = serde_json::from_str(line).map_err(|_| "an answer that does not read".to_string())?;
                let (sealer, opener) = joining.finish(token, &answer)?;
                *self = Session::Open { sealer, opener };

                Ok(Taken::Joined)
            }
            Session::Open { sealer, mut opener } => {
                let opened = opener.open(line);
                *self = Session::Open { sealer, opener };

                opened.map(Taken::Message)
            }
            Session::Closed => Ok(Taken::Nothing),
        }
    }

    /// A message as the line to send, past the handshake; None before it.
    pub fn seal(&mut self, message: &Value) -> Option<String> {
        match self {
            Session::Open { sealer, .. } => Some(sealer.seal(message)),
            _ => None,
        }
    }

    /// Whether it is past the handshake.
    pub fn open(&self) -> bool {
        matches!(self, Session::Open { .. })
    }
}

/// Past the handshake: connected, with no trouble, and nothing yet said of what it mines for.
pub fn joined(status: &mut WorkerStatus) {
    (status.connected, status.trouble, status.mode) = (true, None, None);
}

/// Takes a message from the node into the status, as chainv4's worker does: work, with the span
/// of nonces it gives, for the devices to mine; nothing else for them. An error for a node that
/// refused this worker, or work that does not read, which ends the connection.
pub fn heard(status: &mut WorkerStatus, message: &Value) -> Result<Option<(Work, u64)>, String> {
    match message["type"].as_str() {
        Some("work") => {
            let work = Work::read(message).ok_or("work that does not read")?;
            status.jobs += 1;
            (status.difficulty, status.share_difficulty) = (Some(work.difficulty), work.share);
            // A node that said nothing of a pool or of mining alone mines for itself.
            status.mode.get_or_insert(Mode::Node);
            log::debug!("[miner] job {} at difficulty {}, nonces from {}", work.job, work.difficulty, work.start);

            return Ok(Some((work, message["span"].as_u64().unwrap_or(SPAN))));
        }
        Some("verdict") if message["took"] == true && message["kind"] == "share" => status.shares += 1,
        Some("verdict") if message["took"] == true => {
            status.blocks += 1;
            log::info!("[miner] the node took the nonce for job {}: a block", message["job"]);
        }
        Some("verdict") => {
            status.refused += 1;
            log::info!("[miner] the node refused the nonce for job {}: {}", message["job"], message["why"].as_str().unwrap_or(""));
        }
        Some("pool") => {
            let (fee, window) = (message["fee"].as_u64().unwrap_or(0), message["window"].as_u64().unwrap_or(0));
            status.mode = Some(Mode::Pool { fee, window });
        }
        Some("solo") => status.mode = Some(Mode::Solo),
        Some("refused") => return Err(format!("the node refused this worker: {}", message["why"].as_str().unwrap_or(""))),
        Some("standing") => status.standing = Some(message.clone()),
        // Pings, a release the wallet hears of from its node anyway, and a newer node's messages
        // this miner does not know.
        _ => {}
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use {super::*, chainv4::consensus::meets, chainv4::crypto::hasher::Hasher};

    /// Every work hash a genesis may name, as the wallet's build has them.
    const HASHES: [HashType; 5] = [HashType::Sha256, HashType::Blake3_256, HashType::Blake2s256, HashType::Blake2b256, HashType::Keccak256];

    fn id(hash: HashType, seed: u8) -> Hash {
        Hash::from_bytes(hash, &[seed; 32]).unwrap()
    }

    fn work(share: Option<u128>) -> Work {
        Work { job: 7, id: id(HashType::Blake3_256, 9), difficulty: 64, hash: HashType::Blake3_256, start: 1 << 60, share }
    }

    #[test]
    fn a_range_hands_out_its_nonces_once_each_up_to_its_end() {
        let mut range = Range::new(100, 1_000);
        let mut next = 100;

        while let Some((first, count)) = range.take(300) {
            assert_eq!(first, next, "each picks up where the last left off");
            next += count;
        }

        assert_eq!(next, 1_100, "all of it, and no further");
        assert_eq!(range.take(1), None);

        // A range near the top stops there rather than coming round.
        let mut top = Range::new(u64::MAX - 10, SPAN);

        assert_eq!(top.take(100), Some((u64::MAX - 10, 10)));
        assert_eq!(top.take(100), None);
    }

    #[test]
    fn a_chunk_and_what_was_found_in_it_read_as_written() {
        let chunk = Chunk { generation: 3, hash: HashType::Sha256, id: id(HashType::Blake2b256, 4), difficulty: u128::MAX, start: u64::MAX - 5, count: 5, shares: true };
        let searched = Searched { generation: 3, tried: 5, found: vec![u64::MAX - 1, 2] };

        assert_eq!(Chunk::read(&chunk.to_json()), Some(chunk));
        assert_eq!(Searched::read(&searched.to_json()), Some(searched));
        assert_eq!(Chunk::read(READY), None);
        assert_eq!(Searched::read("{\"generation\": 1}"), None);
    }

    #[test]
    fn a_chunk_finds_what_the_node_checks_for() {
        for hash in HASHES {
            let id = id(hash, 5);
            let meeting: Vec<u64> = (0..2_000).filter(|nonce| meets(work_hash(hash, &id, *nonce), 50)).collect();
            assert!(meeting.len() > 2, "{}: some meet one in fifty", hash.name());

            // A block: the first that meets it, and nothing tried past it.
            let chunk = Chunk { generation: 1, hash, id, difficulty: 50, start: 0, count: 2_000, shares: false };
            let block = chunk.search();

            assert_eq!(block.found, vec![meeting[0]], "{}", hash.name());
            assert_eq!(block.tried, meeting[0] + 1, "{}", hash.name());

            // Shares: every one.
            let shares = Chunk { shares: true, ..chunk }.search();

            assert_eq!(shares.found, meeting, "{}", hash.name());
            assert_eq!(shares.tried, 2_000, "{}", hash.name());
        }
    }

    #[test]
    fn the_gpus_prefix_and_target_hash_and_hold_as_the_node_does() {
        for hash in HASHES {
            let job = Job::new(&Work { hash, id: id(hash, 6), ..work(None) }, SPAN, 1);
            let prefix = job.prefix().unwrap();

            for nonce in [0u64, 1, 1 << 40, u64::MAX] {
                let message = [&prefix[..], &nonce.to_le_bytes()].concat();
                let digest: [u8; 32] = Hasher::digest(hash, &message).to_bytes().try_into().unwrap();

                assert_eq!(u256::from_be_bytes(digest), work_hash(hash, &job.id, nonce), "{} at {nonce}", hash.name());
            }
        }

        // The target holds a hash as meets does, at the edge and either side of it.
        for difficulty in [0u128, 1, 2, 3, 1_000, u128::MAX] {
            let job = Job { difficulty, ..Job::new(&work(None), SPAN, 1) };
            let target = u256::from_be_bytes(job.target());

            for work in [target - 1, target, target.saturating_add(u256::ONE), u256::ZERO, u256::MAX] {
                assert_eq!(work <= target, meets(work, difficulty), "difficulty {difficulty}, work {work}");
            }
        }
    }

    #[test]
    fn a_block_ends_the_job_and_shares_do_not() {
        let mut block = Job::new(&work(None), SPAN, 2);

        assert_eq!(block.chunk(10).map(|chunk| (chunk.start, chunk.count, chunk.difficulty, chunk.shares)), Some((1 << 60, 10, 64, false)));
        assert_eq!(block.found(1, &[5]), Vec::<u64>::new(), "an older job's");
        assert_eq!(block.found(2, &[11, 12]), vec![11], "the first alone");
        assert_eq!(block.found(2, &[13]), Vec::<u64>::new(), "the job is done");
        assert_eq!(block.chunk(10), None, "and hands out no more");

        let mut shares = Job::new(&work(Some(8)), SPAN, 3);

        assert_eq!(shares.chunk(10).map(|chunk| (chunk.difficulty, chunk.shares)), Some((8, true)));
        assert_eq!(shares.found(3, &[11, 12]), vec![11, 12]);
        assert_eq!(shares.found(3, &[13]), vec![13]);
        assert_eq!(shares.chunk(10).map(|chunk| chunk.start), Some((1 << 60) + 10), "it goes on");
    }

    #[test]
    fn a_chunk_grows_or_shrinks_toward_a_tenth_of_a_second_within_bounds() {
        assert_eq!(next_count(10_000, 100.0), 10_000);
        assert_eq!(next_count(10_000, 50.0), 20_000);
        assert_eq!(next_count(10_000, 1.0), 40_000, "four times at most");
        assert_eq!(next_count(10_000, 10_000.0), 2_500, "a quarter at least");
        assert_eq!(next_count(LEAST_CHUNK, 10_000.0), LEAST_CHUNK);
        assert_eq!(next_count(MOST_CHUNK, 0.0), MOST_CHUNK);
    }

    #[test]
    fn a_meter_takes_the_rate_over_a_few_seconds() {
        let start = Instant::now();
        let mut meter = Meter::new(start);
        meter.add(3_000);

        assert_eq!(meter.rate(start + Duration::from_secs(1)), 0.0, "not yet a stretch");
        assert_eq!(meter.rate(start + Duration::from_secs(3)), 1_000.0);

        meter.add(600);

        assert_eq!(meter.rate(start + Duration::from_secs(6)), 200.0, "the last stretch alone");
    }

    #[test]
    fn a_session_shakes_hands_with_the_port_and_seals_both_ways() {
        let (token, me) = (b"a port's token".as_slice(), Hello { name: "browser".to_string(), payout: None, solo: true });
        let mut session = Session::Greeting;
        let (greeting, challenge) = protocol::greeting("chainv4").unwrap();

        assert!(session.seal(&json!({ "type": "rate" })).is_none(), "nothing sealed before the handshake");

        let Taken::Hello(hello) = session.take(&greeting.to_string(), token, "chainv4", &me).unwrap() else {
            panic!("a hello for the greeting");
        };
        let (said, answer, mut sealer, mut opener) = protocol::welcome(token, "chainv4", &challenge, &serde_json::from_str(&hello).unwrap()).unwrap();

        assert_eq!((said.name.as_str(), said.solo), ("browser", true));
        assert_eq!(session.take(&answer.to_string(), token, "chainv4", &me).unwrap(), Taken::Joined);
        assert!(session.open());

        let ping = json!({ "type": "ping" });

        assert_eq!(session.take(&sealer.seal(&ping), token, "chainv4", &me).unwrap(), Taken::Message(ping));

        let found = json!({ "type": "found", "job": 1, "nonce": 5 });

        assert_eq!(opener.open(&session.seal(&found).unwrap()).unwrap(), found);
        assert!(session.take(&format!("00{}", sealer.seal(&json!({}))), token, "chainv4", &me).is_err(), "a line that does not check");

        // A port of another protocol, or one that does not know the token, is refused.
        let (greeting, _) = protocol::greeting("other").unwrap();

        assert!(Session::Greeting.take(&greeting.to_string(), token, "chainv4", &me).is_err());

        let mut session = Session::Greeting;
        let (greeting, _) = protocol::greeting("chainv4").unwrap();
        session.take(&greeting.to_string(), token, "chainv4", &me).unwrap();
        let forged = json!({ "proof": to_hex(&[0; 32]) });

        assert!(session.take(&forged.to_string(), token, "chainv4", &me).unwrap_err().contains("token"));
        assert_eq!(Session::Closed.take("anything", token, "chainv4", &me).unwrap(), Taken::Nothing);
    }

    #[test]
    fn the_nodes_messages_move_the_status_as_chainv4s_worker_does() {
        let mut status = WorkerStatus { trouble: Some("gone".to_string()), ..WorkerStatus::default() };
        joined(&mut status);

        assert!(status.connected && status.trouble.is_none() && status.mode.is_none());

        let mined = work(Some(8));
        let message = json!({
            "type": "work",
            "job": mined.job,
            "chain": mined.id.algorithm().name(),
            "id": to_hex(&mined.id.to_bytes()),
            "difficulty": "64",
            "hash": "blake3",
            "start": mined.start,
            "span": 1_000,
            "share": "8",
        });
        let (taken, span) = heard(&mut status, &message).unwrap().unwrap();

        assert_eq!((taken.job, taken.id, taken.start, taken.share, span), (7, mined.id, 1 << 60, Some(8), 1_000));
        assert_eq!((status.jobs, status.difficulty, status.share_difficulty, status.mode.clone()), (1, Some(64), Some(8), Some(Mode::Node)));

        let mut unspanned = message.clone();
        unspanned.as_object_mut().unwrap().remove("span");

        assert_eq!(heard(&mut status, &unspanned).unwrap().unwrap().1, SPAN, "the node's span when it names none");

        for message in [
            json!({ "type": "pool", "fee": 20_000_000, "window": 8 }),
            json!({ "type": "verdict", "took": true, "kind": "share", "job": 7 }),
            json!({ "type": "verdict", "took": true, "kind": "block", "job": 7 }),
            json!({ "type": "verdict", "took": false, "kind": "share", "job": 7, "why": "late" }),
            json!({ "type": "standing", "shares": 1 }),
            json!({ "type": "ping" }),
        ] {
            assert!(heard(&mut status, &message).unwrap().is_none());
        }

        assert_eq!(status.mode, Some(Mode::Pool { fee: 20_000_000, window: 8 }));
        assert_eq!((status.shares, status.blocks, status.refused), (1, 1, 1));
        assert_eq!(status.standing, Some(json!({ "type": "standing", "shares": 1 })));

        heard(&mut status, &json!({ "type": "solo" })).unwrap();

        assert_eq!(status.mode, Some(Mode::Solo));
        assert!(heard(&mut status, &json!({ "type": "refused", "why": "no payout" })).err().unwrap().contains("no payout"));
        assert!(heard(&mut status, &json!({ "type": "work", "job": 1 })).is_err(), "work that does not read");
    }
}
