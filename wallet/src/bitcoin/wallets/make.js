// PSBTs as a Bitcoin wallet watching the signer makes them, with bitcoinjs-lib, as BlueWallet
// does, and Keystone's registry for their codes: what wallet/src/bitcoin/wallets.json holds, with
// bitcoinjs-lib's own signatures to match. In this folder: npm install, then
// node make.js > ../wallets.json
const ecc = require("@bitcoinerlab/secp256k1");
const { BIP32Factory } = require("bip32");
const bip39 = require("bip39");
const bitcoin = require("bitcoinjs-lib");
const { CryptoPSBT } = require("@keystonehq/bc-ur-registry");

bitcoin.initEccLib(ecc);
const bip32 = BIP32Factory(ecc);
const PHRASE = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const seed = bip39.mnemonicToSeedSync(PHRASE);
const root = bip32.fromSeed(seed);
// Someone else's keys, to pay.
const other = bip32.fromSeed(bip39.mnemonicToSeedSync("test test test test test test test test test test test junk"));
const hex = (bytes) => Buffer.from(bytes).toString("hex");

function parts(psbt) {
    const encoder = new CryptoPSBT(psbt.toBuffer()).toUREncoder(200);
    const out = [];

    for (let i = 0; i < encoder.fragmentsLength * 3; i++) {
        out.push(encoder.nextPart());
    }

    return out;
}

function network(name, net, coin) {
    // The account's key written as the network writes it: xpub…, or tpub… on testnet.
    const account = bip32.fromSeed(seed, net).derivePath(`m/84'/${coin}'/0'`);
    const key = (chain, index) => account.derive(chain).derive(index);
    const wpkh = (pubkey) => bitcoin.payments.p2wpkh({ pubkey, network: net });
    const derivation = (chain, index) => [{ masterFingerprint: root.fingerprint, path: `m/84'/${coin}'/0'/${chain}/${index}`, pubkey: key(chain, index).publicKey }];

    // The transaction the coins came in, with a witness, as most do now.
    const previous = new bitcoin.Transaction();
    previous.version = 2;
    previous.addInput(Buffer.alloc(32, 7), 0);
    previous.addOutput(wpkh(key(0, 0).publicKey).output, 50000);
    previous.addOutput(wpkh(key(0, 1).publicKey).output, 30000);
    previous.setWitness(0, [Buffer.alloc(71, 1), Buffer.alloc(33, 2)]);

    const theirs = other.derivePath("m/84'/0'/0'/0/0").publicKey;
    const outputs = [
        { label: "native segwit", script: wpkh(theirs).output, value: 20000 },
        { label: "taproot", script: bitcoin.payments.p2tr({ internalPubkey: theirs.subarray(1), network: net }).output, value: 10000 },
        { label: "legacy", script: bitcoin.payments.p2pkh({ pubkey: theirs, network: net }).output, value: 5000 },
        { label: "script hash", script: bitcoin.payments.p2sh({ redeem: bitcoin.payments.p2wpkh({ pubkey: theirs, network: net }), network: net }).output, value: 5000 },
        { label: "data", script: bitcoin.payments.embed({ data: [Buffer.from("chainv4")] }).output, value: 0 },
    ];

    function build(proven) {
        const psbt = new bitcoin.Psbt({ network: net });

        for (const index of [0, 1]) {
            psbt.addInput({
                hash: previous.getId(),
                index,
                sequence: 0xfffffffd,
                witnessUtxo: { script: previous.outs[index].script, value: previous.outs[index].value },
                ...(proven ? { nonWitnessUtxo: previous.toBuffer() } : {}),
                bip32Derivation: derivation(0, index),
            });
        }

        for (const { script, value } of outputs) {
            psbt.addOutput({ script, value });
        }

        psbt.addOutput({ address: wpkh(key(1, 0).publicKey).address, value: 38000, bip32Derivation: derivation(1, 0) });

        return psbt;
    }

    const psbt = build(true);
    const signed = build(true);
    signed.signAllInputsHD(root);

    return {
        name,
        account: account.neutered().toBase58(),
        receiving: [0, 1].map((index) => wpkh(key(0, index).publicKey).address),
        change: wpkh(key(1, 0).publicKey).address,
        outputs: outputs.map(({ label, script }) => ({ label, script: hex(script), address: label === "data" ? null : bitcoin.address.fromOutputScript(script, net) })),
        psbt: hex(psbt.toBuffer()),
        parts: parts(psbt),
        witness_only: hex(build(false).toBuffer()),
        // What bitcoinjs-lib signs each input with: RFC 6979, as the signer does, so the same.
        signatures: signed.data.inputs.map((input) => hex(input.partialSig[0].signature)),
        transaction: hex(psbt.data.globalMap.unsignedTx.toBuffer()),
    };
}

console.log(
    JSON.stringify(
        { xfp: hex(root.fingerprint), main: network("main", bitcoin.networks.bitcoin, 0), test: network("test", bitcoin.networks.testnet, 1) },
        null,
        2,
    ),
);
