// What every work-hash kernel shares: the job, the result, and the search over nonces.
//
// A work hash is the chain's hash over "chainv4 work", the block's id and the nonce, 52 bytes
// in all, read as a big-endian number; a nonce seals the block when that is at most the target.
// The job carries the first 44 bytes as little-endian words, the target as big-endian words, and
// a range of nonces. Each invocation hashes one nonce and, when it meets the target, records its
// index; the lowest wins, so the result is the one the CPU would have found first.

struct Job {
    prefix: array<u32, 11>,
    bound: array<u32, 8>,
    start_lo: u32,
    start_hi: u32,
    count: u32,
    pad: u32,
};

@group(0) @binding(0) var<storage, read> job: Job;
@group(0) @binding(1) var<storage, read_write> found: array<atomic<u32>, 1>;

fn bswap(x: u32) -> u32 {
    return ((x & 0xffu) << 24u) | ((x & 0xff00u) << 8u) | ((x >> 8u) & 0xff00u) | (x >> 24u);
}

fn rotr(x: u32, n: u32) -> u32 {
    return (x >> n) | (x << (32u - n));
}

// Whether a digest, as big-endian words, is at most the target.
fn meets(d: ptr<function, array<u32, 8>>) -> bool {
    for (var k = 0u; k < 8u; k++) {
        let t = job.bound[k];

        if ((*d)[k] < t) {
            return true;
        }

        if ((*d)[k] > t) {
            return false;
        }
    }

    return true;
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;

    if (i >= job.count) {
        return;
    }

    let lo = job.start_lo + i;
    let hi = job.start_hi + select(0u, 1u, lo < job.start_lo);
    var m: array<u32, 16>;

    for (var k = 0u; k < 11u; k++) {
        m[k] = job.prefix[k];
    }

    m[11] = lo;
    m[12] = hi;
    m[13] = 0u;
    m[14] = 0u;
    m[15] = 0u;

    var d: array<u32, 8>;
    digest(&m, &d);

    if (meets(&d)) {
        atomicMin(&found[0], i + 1u);
    }
}
