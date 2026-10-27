// The wallet's codes, read back as a Bitcoin wallet reads them: the pairing with Keystone's
// registry, the signed PSBTs with it and bitcoinjs-lib, as BlueWallet does. In this folder, after
// npm install: CHAINV4_BITCOIN_OUT=$PWD/ours.json cargo test --lib bitcoin_wallets_own, then
// node check.js.
const ecc = require("@bitcoinerlab/secp256k1");
const { BIP32Factory } = require("bip32");
const bitcoin = require("bitcoinjs-lib");
const { CryptoAccount, CryptoPSBT, ScriptExpressions, URRegistryDecoder } = require("@keystonehq/bc-ur-registry");

bitcoin.initEccLib(ecc);
const bip32 = BIP32Factory(ecc);
const made = require("../wallets.json");
const ours = require("./ours.json");
let failed = 0;
const check = (what, ok) => {
    console.log(`${ok ? "ok  " : "FAIL"} ${what}`);
    failed += ok ? 0 : 1;
};
const read = (parts) => {
    const decoder = new URRegistryDecoder();

    for (const part of parts) {
        if (decoder.isComplete()) break;
        decoder.receivePart(part.toLowerCase());
    }

    return decoder.isComplete() ? decoder.resultRegistryType() : null;
};

for (const [name, net] of [["main", bitcoin.networks.bitcoin], ["test", bitcoin.networks.testnet]]) {
    const expected = made[name];

    // Pairing: the account's one output, wpkh() of its key at m/84'/coin'/0'.
    const account = read([ours[name].pairing]);
    check(`${name}: pairing is a crypto-account`, account instanceof CryptoAccount);
    check(`${name}: the seed's fingerprint`, account.getMasterFingerprint().toString("hex") === made.xfp);
    const [output] = account.getOutputDescriptors();
    check(`${name}: native segwit`, output.getScriptExpressions().map((e) => e.getTag()).join() === `${ScriptExpressions.WITNESS_PUBLIC_KEY_HASH.getTag()}`);
    const key = output.getHDKey();
    const coin = name === "main" ? 0 : 1;
    check(`${name}: at m/84'/${coin}'/0'`, key.getOrigin().getPath() === `84'/${coin}'/0'`);
    check(`${name}: the network`, key.getUseInfo().getNetwork() === coin);
    // Keystone's library writes any key as xpub…, whatever its network says: the key and chain
    // code are what count.
    const node = bip32.fromBase58(key.getBip32Key(), bitcoin.networks.bitcoin);
    const wanted = bip32.fromBase58(expected.account, net);
    check(`${name}: the key bitcoinjs derives the account with`, node.publicKey.equals(wanted.publicKey) && node.chainCode.equals(wanted.chainCode));
    const at = (chain, index) => bitcoin.payments.p2wpkh({ pubkey: node.derive(chain).derive(index).publicKey, network: net }).address;
    check(`${name}: its first address, ${at(0, 0)}`, at(0, 0) === expected.receiving[0]);
    check(`${name}: its change`, at(1, 0) === expected.change);

    // Signed PSBTs, animated, read as the wallet reads them, finished and sent.
    for (const signed of ours[name].signed) {
        const decoded = read(signed.parts);
        check(`${name} ${signed.name}: a crypto-psbt off its parts`, decoded instanceof CryptoPSBT);
        check(`${name} ${signed.name}: the same off the whole code`, read([signed.whole]).getPSBT().equals(decoded.getPSBT()));
        const psbt = bitcoin.Psbt.fromBuffer(decoded.getPSBT(), { network: net });
        const asked = bitcoin.Psbt.fromBuffer(Buffer.from(expected[signed.name === "psbt" ? "psbt" : "witness_only"], "hex"), { network: net });
        check(`${name} ${signed.name}: each input signed`, psbt.data.inputs.every((input) => input.partialSig?.length === 1));
        check(`${name} ${signed.name}: the signatures hold`, psbt.validateSignaturesOfAllInputs((pubkey, hash, signature) => ecc.verify(hash, pubkey, signature)));
        psbt.finalizeAllInputs();
        const tx = psbt.extractTransaction();
        check(`${name} ${signed.name}: the transaction asked for`, tx.getId() === asked.data.globalMap.unsignedTx.tx.getId());
        check(`${name} ${signed.name}: its fee, 2000`, psbt.getFee() === 2000);
        console.log(`     ${tx.virtualSize()} vB, ${tx.toHex().length / 2} bytes`);
    }
}

console.log(failed ? `${failed} failed` : "all ok");
process.exit(failed ? 1 : 0);
