// SPDX-License-Identifier: MIT
// Ed25519 signatures checked in Solidity, as RFC 8032 checks them: what MultiSig.sol falls back on
// for an Ed25519 owner on a chain without chainv4's crypto precompile. Cofactorless, as
// ed25519-dalek's verify is: [S]B = R + [k]A, compared by R's encoding, S below the group's order.
// It costs about 400,000 gas; on chainv4 the precompile does the same for a few thousand.
//
// SHA-512 is written out here too, for any length, since the EVM has none. Points are kept in
// extended coordinates (X:Y:Z:T), x = X/Z, y = Y/Z, xy = T/Z, with RFC 8032's addition and
// doubling for a = -1, and every field operation is the EVM's own addmod and mulmod.
pragma solidity ^0.8.24;

library Ed25519 {
    uint256 private constant P = 0x7fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffed;
    /// d = -121665/121666, the curve's constant.
    uint256 private constant D = 0x52036cee2b6ffe738cc740797779e89800700a4d4141d8ab75eb4dca135978a3;
    /// 2d, which addition takes.
    uint256 private constant D2 = 0x2406d9dc56dffce7198e80f2eef3d13000e0149a8283b156ebd69b9426b2f159;
    /// The order of the base point's group.
    uint256 private constant L = 0x1000000000000000000000000000000014def9dea2f79cd65812631a5cf5d3ed;
    /// 2^256 mod L, for reducing a 512-bit hash.
    uint256 private constant R256 = 0x0ffffffffffffffffffffffffffffffec6ef5bf4737dcf70d6ec31748d98951d;
    /// A square root of -1.
    uint256 private constant SQRT_M1 = 0x2b8324804fc1df0b2b4d00993dfbd7a72f431806ad2fe478c4ee1b274a0ea0b0;
    /// The base point.
    uint256 private constant BX = 0x216936d3cd6e53fec0a4e231fdd6dc5c692cc7609525a7b2c9562d608f25d51a;
    uint256 private constant BY = 0x6666666666666666666666666666666666666666666666666666666666666658;

    /// Whether signature, R then S as RFC 8032 writes them, is key's over message. Bad encodings
    /// are false, never a revert.
    function verify(bytes32 key, bytes32 r, bytes32 s, bytes memory message) internal pure returns (bool) {
        uint256 scalar = uint256(reverse(s));

        if (scalar >= L) {
            return false;
        }

        (bool ok, uint256[4] memory a) = decode(key);

        if (!ok) {
            return false;
        }

        uint256 k = reduce(sha512(abi.encodePacked(r, key, message)));
        // -A: x and t negated.
        a[0] = P - a[0];
        a[3] = P - a[3];
        uint256[4] memory b = [BX, BY, 1, mulmod(BX, BY, P)];
        // What the loop adds, each cached as (Y-X, Y+X, 2dT, 2Z): B, -A and B - A, one after another.
        uint256[12] memory cached;
        cache(cached, 0, b);
        cache(cached, 4, a);
        cache(cached, 8, add(b, a));
        uint256[4] memory point;

        assembly {
            let p := P

            // RFC 8032's doubling for a = -1; it reads no T.
            function dbl(x1, y1, z1, q) -> x3, y3, z3, t3 {
                let a_ := mulmod(x1, x1, q)
                let b_ := mulmod(y1, y1, q)
                let h := addmod(a_, b_, q)
                let s_ := addmod(x1, y1, q)
                let e := addmod(h, sub(q, mulmod(s_, s_, q)), q)
                let g := addmod(a_, sub(q, b_), q)
                let f := addmod(mulmod(2, mulmod(z1, z1, q), q), g, q)
                x3 := mulmod(e, f, q)
                y3 := mulmod(g, h, q)
                z3 := mulmod(f, g, q)
                t3 := mulmod(e, h, q)
            }

            // Adds a cached point, at c in memory, to (x1, y1, z1, t1).
            function addCached(x1, y1, z1, t1, c, q) -> x3, y3, z3, t3 {
                let a_ := mulmod(addmod(y1, sub(q, x1), q), mload(c), q)
                let b_ := mulmod(addmod(y1, x1, q), mload(add(c, 32)), q)
                let e := addmod(b_, sub(q, a_), q)
                let h := addmod(b_, a_, q)
                let c_ := mulmod(t1, mload(add(c, 64)), q)
                let d_ := mulmod(z1, mload(add(c, 96)), q)
                let f := addmod(d_, sub(q, c_), q)
                let g := addmod(d_, c_, q)
                x3 := mulmod(e, f, q)
                y3 := mulmod(g, h, q)
                z3 := mulmod(f, g, q)
                t3 := mulmod(e, h, q)
            }

            // [scalar]B + [k](-A), from the top bit down, starting at the identity.
            let x := 0
            let y := 1
            let z := 1
            let t := 0

            for { let i := 256 } gt(i, 0) { i := sub(i, 1) } {
                x, y, z, t := dbl(x, y, z, p)
                let bits := or(and(shr(sub(i, 1), scalar), 1), shl(1, and(shr(sub(i, 1), k), 1)))

                if bits {
                    x, y, z, t := addCached(x, y, z, t, add(cached, mul(sub(bits, 1), 128)), p)
                }
            }

            mstore(point, x)
            mstore(add(point, 32), y)
            mstore(add(point, 64), z)
            mstore(add(point, 96), t)
        }

        return encode(point) == r;
    }

    /// A point as the loop adds it, (Y-X, Y+X, 2dT, 2Z), into four words of out from from.
    function cache(uint256[12] memory out, uint256 from, uint256[4] memory point) private pure {
        out[from] = addmod(point[1], P - point[0], P);
        out[from + 1] = addmod(point[1], point[0], P);
        out[from + 2] = mulmod(point[3], D2, P);
        out[from + 3] = addmod(point[2], point[2], P);
    }

    /// A point from its 32-byte encoding: y, little-endian, with x's low bit on top.
    function decode(bytes32 encoded) private pure returns (bool, uint256[4] memory point) {
        uint256 value = uint256(reverse(encoded));
        uint256 sign = value >> 255;
        uint256 y = value & ((1 << 255) - 1);

        if (y >= P) {
            return (false, point);
        }

        uint256 yy = mulmod(y, y, P);
        uint256 u = addmod(yy, P - 1, P);
        uint256 v = addmod(mulmod(D, yy, P), 1, P);
        // x = u v^3 (u v^7)^((p-5)/8), RFC 8032's square root of u/v.
        uint256 v3 = mulmod(mulmod(v, v, P), v, P);
        uint256 v7 = mulmod(mulmod(v3, v3, P), v, P);
        uint256 x = mulmod(mulmod(u, v3, P), pow(mulmod(u, v7, P), (P - 5) / 8), P);
        uint256 vxx = mulmod(v, mulmod(x, x, P), P);

        if (vxx != u) {
            if (vxx != P - u) {
                return (false, point);
            }

            x = mulmod(x, SQRT_M1, P);
        }

        if (x == 0 && sign == 1) {
            return (false, point);
        }

        if (x & 1 != sign) {
            x = P - x;
        }

        point = [x, y, 1, mulmod(x, y, P)];

        return (true, point);
    }

    /// A point's 32-byte encoding.
    function encode(uint256[4] memory point) private pure returns (bytes32) {
        uint256 inverse = pow(point[2], P - 2);
        uint256 x = mulmod(point[0], inverse, P);
        uint256 y = mulmod(point[1], inverse, P);

        return reverse(bytes32(y | ((x & 1) << 255)));
    }

    function add(uint256[4] memory p1, uint256[4] memory p2) private pure returns (uint256[4] memory) {
        uint256 a = mulmod(addmod(p1[1], P - p1[0], P), addmod(p2[1], P - p2[0], P), P);
        uint256 b = mulmod(addmod(p1[1], p1[0], P), addmod(p2[1], p2[0], P), P);
        uint256 c = mulmod(mulmod(p1[3], D2, P), p2[3], P);
        uint256 d = mulmod(addmod(p1[2], p1[2], P), p2[2], P);
        uint256 e = addmod(b, P - a, P);
        uint256 f = addmod(d, P - c, P);
        uint256 g = addmod(d, c, P);
        uint256 h = addmod(b, a, P);

        return [mulmod(e, f, P), mulmod(g, h, P), mulmod(f, g, P), mulmod(e, h, P)];
    }

    function pow(uint256 base, uint256 exponent) private pure returns (uint256 result) {
        assembly {
            let p := P
            result := 1

            for {} exponent { exponent := shr(1, exponent) } {
                if and(exponent, 1) {
                    result := mulmod(result, base, p)
                }

                base := mulmod(base, base, p)
            }
        }
    }

    /// A 64-byte hash as a little-endian number, mod L.
    function reduce(bytes memory hash) private pure returns (uint256) {
        bytes32 low;
        bytes32 high;

        assembly {
            low := mload(add(hash, 32))
            high := mload(add(hash, 64))
        }

        return addmod(mulmod(uint256(reverse(high)) % L, R256, L), uint256(reverse(low)) % L, L);
    }

    /// The 32 bytes in the other order.
    function reverse(bytes32 input) private pure returns (bytes32) {
        uint256 v = uint256(input);
        v = ((v & 0xff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00) >> 8) | ((v & 0x00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff) << 8);
        v = ((v & 0xffff0000ffff0000ffff0000ffff0000ffff0000ffff0000ffff0000ffff0000) >> 16) | ((v & 0x0000ffff0000ffff0000ffff0000ffff0000ffff0000ffff0000ffff0000ffff) << 16);
        v = ((v & 0xffffffff00000000ffffffff00000000ffffffff00000000ffffffff00000000) >> 32) | ((v & 0x00000000ffffffff00000000ffffffff00000000ffffffff00000000ffffffff) << 32);
        v = ((v & 0xffffffffffffffff0000000000000000ffffffffffffffff0000000000000000) >> 64) | ((v & 0x0000000000000000ffffffffffffffff0000000000000000ffffffffffffffff) << 64);
        v = (v >> 128) | (v << 128);

        return bytes32(v);
    }

    /// SHA-512 of any number of bytes.
    function sha512(bytes memory message) internal pure returns (bytes memory out) {
        // The message, a 1 bit, zeros, and its length in bits as 128 bits: whole 128-byte blocks.
        uint256 length = message.length;
        uint256 blocks = (length + 17 + 127) / 128;
        bytes memory padded = new bytes(blocks * 128);
        uint64[80] memory k = constants();
        // The state, the schedule and the working variables, one 64-bit number a word.
        uint256[8] memory h = [
            uint256(0x6a09e667f3bcc908), 0xbb67ae8584caa73b, 0x3c6ef372fe94f82b, 0xa54ff53a5f1d36f1,
            0x510e527fade682d1, 0x9b05688c2b3e6c1f, 0x1f83d9abfb41bd6b, 0x5be0cd19137e2179
        ];
        uint256[80] memory w;
        uint256[8] memory v;
        // Room for the last 8-byte write's word to run past the 64 bytes.
        out = new bytes(96);

        assembly {
            function rotr(x, n) -> r {
                r := and(or(shr(n, x), shl(sub(64, n), x)), 0xffffffffffffffff)
            }

            let mask := 0xffffffffffffffff
            let data := add(padded, 32)

            // The message, a byte at a time, so nothing past it comes along; then the 1 bit; then
            // its length, in the last 16 bytes, leaving the bytes before them as they are.
            for { let i := 0 } lt(i, length) { i := add(i, 1) } {
                mstore8(add(data, i), byte(0, mload(add(add(message, 32), i))))
            }

            mstore8(add(data, length), 0x80)
            let end := add(data, mload(padded))
            let last := mload(sub(end, 32))
            mstore(sub(end, 32), or(and(last, not(0xffffffffffffffffffffffffffffffff)), mul(length, 8)))

            for { let offset := 0 } lt(offset, mload(padded)) { offset := add(offset, 128) } {
                for { let t := 0 } lt(t, 16) { t := add(t, 1) } {
                    mstore(add(w, mul(t, 32)), shr(192, mload(add(data, add(offset, mul(t, 8))))))
                }

                for { let t := 16 } lt(t, 80) { t := add(t, 1) } {
                    let w15 := mload(add(w, mul(sub(t, 15), 32)))
                    let w2 := mload(add(w, mul(sub(t, 2), 32)))
                    let s0 := xor(xor(rotr(w15, 1), rotr(w15, 8)), shr(7, w15))
                    let s1 := xor(xor(rotr(w2, 19), rotr(w2, 61)), shr(6, w2))
                    mstore(add(w, mul(t, 32)), and(add(add(add(mload(add(w, mul(sub(t, 16), 32))), s0), mload(add(w, mul(sub(t, 7), 32)))), s1), mask))
                }

                for { let i := 0 } lt(i, 8) { i := add(i, 1) } {
                    mstore(add(v, mul(i, 32)), mload(add(h, mul(i, 32))))
                }

                for { let t := 0 } lt(t, 80) { t := add(t, 1) } {
                    let e := mload(add(v, 128))
                    let choose := xor(and(e, mload(add(v, 160))), and(xor(e, mask), mload(add(v, 192))))
                    let big1 := xor(xor(rotr(e, 14), rotr(e, 18)), rotr(e, 41))
                    let t1 := add(add(add(add(mload(add(v, 224)), big1), choose), mload(add(k, mul(t, 32)))), mload(add(w, mul(t, 32))))
                    let a := mload(v)
                    let b := mload(add(v, 32))
                    let c := mload(add(v, 64))
                    let big0 := xor(xor(rotr(a, 28), rotr(a, 34)), rotr(a, 39))
                    let t2 := add(big0, xor(xor(and(a, b), and(a, c)), and(b, c)))
                    mstore(add(v, 224), mload(add(v, 192)))
                    mstore(add(v, 192), mload(add(v, 160)))
                    mstore(add(v, 160), e)
                    mstore(add(v, 128), and(add(mload(add(v, 96)), t1), mask))
                    mstore(add(v, 96), c)
                    mstore(add(v, 64), b)
                    mstore(add(v, 32), a)
                    mstore(v, and(add(t1, t2), mask))
                }

                for { let i := 0 } lt(i, 8) { i := add(i, 1) } {
                    let slot := add(h, mul(i, 32))
                    mstore(slot, and(add(mload(slot), mload(add(v, mul(i, 32)))), mask))
                }
            }

            // The state, big-endian, 8 bytes each; each word's zeros overwritten by the next.
            for { let i := 0 } lt(i, 8) { i := add(i, 1) } {
                mstore(add(add(out, 32), mul(i, 8)), shl(192, mload(add(h, mul(i, 32)))))
            }

            mstore(out, 64)
        }
    }

    function constants() private pure returns (uint64[80] memory) {
        return [
            0x428a2f98d728ae22, 0x7137449123ef65cd, 0xb5c0fbcfec4d3b2f, 0xe9b5dba58189dbbc, 0x3956c25bf348b538,
            0x59f111f1b605d019, 0x923f82a4af194f9b, 0xab1c5ed5da6d8118, 0xd807aa98a3030242, 0x12835b0145706fbe,
            0x243185be4ee4b28c, 0x550c7dc3d5ffb4e2, 0x72be5d74f27b896f, 0x80deb1fe3b1696b1, 0x9bdc06a725c71235,
            0xc19bf174cf692694, 0xe49b69c19ef14ad2, 0xefbe4786384f25e3, 0x0fc19dc68b8cd5b5, 0x240ca1cc77ac9c65,
            0x2de92c6f592b0275, 0x4a7484aa6ea6e483, 0x5cb0a9dcbd41fbd4, 0x76f988da831153b5, 0x983e5152ee66dfab,
            0xa831c66d2db43210, 0xb00327c898fb213f, 0xbf597fc7beef0ee4, 0xc6e00bf33da88fc2, 0xd5a79147930aa725,
            0x06ca6351e003826f, 0x142929670a0e6e70, 0x27b70a8546d22ffc, 0x2e1b21385c26c926, 0x4d2c6dfc5ac42aed,
            0x53380d139d95b3df, 0x650a73548baf63de, 0x766a0abb3c77b2a8, 0x81c2c92e47edaee6, 0x92722c851482353b,
            0xa2bfe8a14cf10364, 0xa81a664bbc423001, 0xc24b8b70d0f89791, 0xc76c51a30654be30, 0xd192e819d6ef5218,
            0xd69906245565a910, 0xf40e35855771202a, 0x106aa07032bbd1b8, 0x19a4c116b8d2d0c8, 0x1e376c085141ab53,
            0x2748774cdf8eeb99, 0x34b0bcb5e19b48a8, 0x391c0cb3c5c95a63, 0x4ed8aa4ae3418acb, 0x5b9cca4f7763e373,
            0x682e6ff3d6b2b8a3, 0x748f82ee5defb2fc, 0x78a5636f43172f60, 0x84c87814a1f0ab72, 0x8cc702081a6439ec,
            0x90befffa23631e28, 0xa4506cebde82bde9, 0xbef9a3f7b2c67915, 0xc67178f2e372532b, 0xca273eceea26619c,
            0xd186b8c721c0c207, 0xeada7dd6cde0eb1e, 0xf57d4f7fee6ed178, 0x06f067aa72176fba, 0x0a637dc5a2c898a6,
            0x113f9804bef90dae, 0x1b710b35131c471b, 0x28db77f523047d84, 0x32caab7b40c72493, 0x3c9ebe0a15c9bebc,
            0x431d67c49c100d4c, 0x4cc5d4becb3e42b6, 0x597f299cfc657e2a, 0x5fcb6fab3ad6faec, 0x6c44198c4a475817
        ];
    }
}
