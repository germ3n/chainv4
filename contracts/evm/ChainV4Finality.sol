// SPDX-License-Identifier: MIT
// chainv4's finality, followed on an EVM chain. chainv4's validators each name an attester, a
// secp256k1 key apart from their voting key, and beside each vote sign a keccak256 commitment of
// the link they vote for: its source and target checkpoints, the target's height and state root,
// and the hashes of the attesting sets of the target's epoch and the next. This contract takes a
// link once attesters holding two thirds of the set's whole weight signed it, as chainv4 justifies
// a checkpoint: it records the target as justified, with its state root, and the source as final
// when the target is the very next epoch and the source's own link was taken, as Casper FFG
// finalizes. Then the set it checks the next link by is the next epoch's, which the signers
// committed to, so it follows the set as it changes.
//
// It need not take every link: any later one its set signed will do, so a relayer skips ahead,
// taking two links in a row now and then to record a checkpoint and then finalize it, and only
// the link where the set changes besides, which saves the gas of a link every epoch.
//
// It trusts what two thirds of chainv4's validators sign, as chainv4's own light clients do: what
// it adds is that nothing else is trusted, no federation and no relayer. Anyone may submit a link.
//
// Finality proofs check the validators' own votes instead of their attesters': an SP1 proof that two
// thirds of an epoch's voters signed a link, the voters as the source checkpoint's state fixed them,
// taken beside the attesters' signatures (proveLink) from any checkpoint recorded here with that
// state root. So the two take turns as they like, and with attestations off Ethereum still follows
// chainv4 by proofs.
//
// Validity proofs check what they sign. A proof, SP1's checked by SP1's verifiers, shows that every
// block from the proven tip to a final checkpoint ran by chainv4's rules, from the tip's state root
// to the checkpoint's: once one is taken, a checkpoint's state root is no longer only the
// validators' word. Each proof also gives the root of what left chainv4 through its gateway in its
// range, which Gateway.sol releases withdrawals by, and the program and gateway mode chainv4's
// governance names, which this contract follows.
//
// Built with: solc --evm-version prague --optimize --via-ir --bin --abi ChainV4Finality.sol
pragma solidity ^0.8.24;

/// SP1's verifier, or its gateway, which routes a proof to the verifier of its version: SP1 keeps a
/// gateway for each wrapping, Groth16 and PLONK.
interface ISP1Verifier {
    function verifyProof(bytes32 programVKey, bytes calldata publicValues, bytes calldata proofBytes) external view;
}

contract ChainV4Finality {
    /// What sets an attestation apart from anything else an attester's key signs.
    bytes32 public constant DOMAIN = keccak256("chainv4 attestation");

    /// secp256k1's order halved: an s above it is the other shape of a signature, refused so that no
    /// attestation has two.
    uint256 private constant HALF_ORDER = 0x7fffffffffffffffffffffffffffffff5d576e7357a4501ddfe92f46681b20a0;

    /// The chain followed, by its EVM chain id, as attestations name it.
    uint256 public immutable chainId;

    /// The hash of the attesting set that signs the next link: keccak256(abi.encode(total, needed,
    /// attesters, weights)), attesters by address ascending, needed the weight a link's votes must
    /// reach as chainv4's governance sets the threshold, never under two thirds.
    bytes32 public setHash;

    /// The finality program finality proofs are by, as chainv4's governance names it, zero for
    /// none, when no finality proof is taken.
    bytes32 public finalityProgram;

    /// The newest justified checkpoint taken, and the newest final one.
    uint256 public justifiedEpoch;
    bytes32 public justifiedBlock;
    uint256 public finalizedEpoch;
    bytes32 public finalizedBlock;

    /// Each justified checkpoint taken, by epoch: its block, height and state root, and whether it
    /// was finalized here.
    mapping(uint256 => bytes32) public blockAt;
    mapping(uint256 => uint256) public heightAt;
    mapping(uint256 => bytes32) public stateRootAt;
    mapping(uint256 => bool) public finalAt;

    /// What checks validity proofs, any one of them: SP1's gateways, one for each kind of proof
    /// this chain takes, set at deployment. Then the genesis id they are of, and the program they
    /// prove, as chainv4's governance names it: zero for none yet, when no proof is taken.
    ISP1Verifier[] public verifiers;
    bytes32 public immutable genesis;
    bytes32 public program;

    /// The proven tip, where the next proof starts: the block, its height and its state root.
    uint256 public provenHeight;
    bytes32 public provenBlock;
    bytes32 public provenRoot;

    /// For each proven range, by the block it ends at, the root of what left chainv4 through its
    /// gateway in it; and how Gateway.sol releases, 0 by a proof or the federation, 1 by both.
    mapping(bytes32 => bytes32) public withdrawalsOf;
    uint8 public gatewayMode;

    /// Where proofs began, and every proven range's end in turn, by height with its block: a range
    /// runs from the end before it, or provenFrom, so a relayer finds the one a transfer left in.
    uint256 public immutable provenFrom;
    uint256[] public provenEnds;
    mapping(uint256 => bytes32) public provenBlockAt;

    event Justified(uint256 indexed epoch, bytes32 block_, uint256 height, bytes32 stateRoot);
    event Finalized(uint256 indexed epoch, bytes32 block_);
    event SetMoved(bytes32 setHash);
    event Proven(uint256 height, bytes32 block_, bytes32 stateRoot, bytes32 withdrawals);
    event ProgramMoved(bytes32 program);
    event FinalityProgramMoved(bytes32 program);

    /// Starts from a checkpoint the deployer trusts as final, and the hash of the attesting set
    /// that signs the next link: what a chainv4 node it trusts says. Proofs start from the proven
    /// point given, which chainv4 keeps as its own proven tip, under the program given, both zero
    /// while chainv4 takes no proofs; and are checked by the SP1 verifiers given, a proof standing
    /// if any of them takes it: none, and no proof is taken.
    constructor(
        uint256 chainId_,
        uint256 epoch,
        bytes32 block_,
        uint256 height,
        bytes32 stateRoot,
        bytes32 setHash_,
        ISP1Verifier[] memory verifiers_,
        bytes32 genesis_,
        bytes32 program_,
        uint256 provenHeight_,
        bytes32 provenBlock_,
        bytes32 provenRoot_,
        bytes32 finalityProgram_
    ) {
        finalityProgram = finalityProgram_;
        chainId = chainId_;
        setHash = setHash_;
        verifiers = verifiers_;
        genesis = genesis_;
        program = program_;
        provenHeight = provenHeight_;
        provenFrom = provenHeight_;
        provenBlock = provenBlock_;
        provenRoot = provenRoot_;
        finalAt[epoch] = true;
        justifiedEpoch = epoch;
        justifiedBlock = block_;
        finalizedEpoch = epoch;
        finalizedBlock = block_;
        blockAt[epoch] = block_;
        heightAt[epoch] = height;
        stateRootAt[epoch] = stateRoot;
    }

    /// The digest attesters sign for a link: keccak256(abi.encode(DOMAIN, chainId, sourceEpoch,
    /// sourceBlock, targetEpoch, targetBlock, height, stateRoot, set, nextSet)).
    function digest(bytes32[6] calldata link, bytes32 set, bytes32 nextSet) public view returns (bytes32) {
        return keccak256(abi.encode(DOMAIN, chainId, link[0], link[1], link[2], link[3], link[4], link[5], set, nextSet));
    }

    /// Takes a link once enough of the attesting set signed it.
    ///   link: sourceEpoch, sourceBlock, targetEpoch, targetBlock, the target's height and state
    ///         root, each a word.
    ///   total, needed, attesters, weights: the attesting set that signs it, whose hash is setHash,
    ///         and the weight its attesters must reach.
    ///   nextSet: the hash of the next epoch's attesting set, which it moves to.
    ///   signers: the places in attesters of those that signed, ascending.
    ///   signatures: their attestations, 65 bytes each, r, s and v, in the same order.
    function submit(
        bytes32[6] calldata link,
        uint256 total,
        uint256 needed,
        address[] calldata attesters,
        uint256[] calldata weights,
        bytes32 nextSet,
        uint256[] calldata signers,
        bytes calldata signatures
    ) external {
        require(attesters.length == weights.length, "one weight for each attester");
        require(keccak256(abi.encode(total, needed, attesters, weights)) == setHash, "not the set that signs the next link");
        require(needed * 3 >= total * 2, "a threshold under two thirds");
        require(link[1] != bytes32(0), "a link from no checkpoint");
        require(uint256(link[2]) > uint256(link[0]) && uint256(link[2]) > justifiedEpoch, "its target is no later checkpoint");
        require(signatures.length == signers.length * 65, "65 bytes for each signer");
        require(signed(digest(link, setHash, nextSet), attesters, weights, signers, signatures) >= needed, "short of the weight a link needs");

        record(link, nextSet);
    }

    /// Takes a link by a finality proof: eleven words of public values, chainId, genesis,
    /// sourceEpoch, sourceBlock, sourceRoot, targetEpoch, targetBlock, the target's height and state
    /// root, the next epoch's attesting set, and the finality program chainv4's governance names at
    /// the target. Its source must be a checkpoint recorded here with that state root, which fixed
    /// the voters that signed it; the attesters' set moves to the next, and the program follows.
    function proveLink(bytes calldata publicValues, bytes calldata proof) external {
        require(finalityProgram != bytes32(0) && verifiers.length > 0, "no finality program to prove by");
        require(publicValues.length == 352, "eleven words of public values");
        bytes32[11] memory v = abi.decode(publicValues, (bytes32[11]));
        require(uint256(v[0]) == chainId && v[1] == genesis, "a link of another chain");
        uint256 sourceEpoch = uint256(v[2]);
        require(v[3] != bytes32(0) && blockAt[sourceEpoch] == v[3] && stateRootAt[sourceEpoch] == v[4], "a link from no checkpoint recorded here");
        require(uint256(v[5]) > sourceEpoch && uint256(v[5]) > justifiedEpoch, "its target is no later checkpoint");
        require(verified(finalityProgram, publicValues, proof), "no verifier here takes the proof");

        record([v[2], v[3], v[5], v[6], v[7], v[8]], v[9]);

        if (v[10] != bytes32(0) && v[10] != finalityProgram) {
            finalityProgram = v[10];
            emit FinalityProgramMoved(v[10]);
        }
    }

    /// The weight of the attesters whose attestations of a digest these are.
    function signed(bytes32 linked, address[] calldata attesters, uint256[] calldata weights, uint256[] calldata signers, bytes calldata signatures) private pure returns (uint256 weight) {
        for (uint256 i = 0; i < signers.length; i++) {
            uint256 place = signers[i];
            require(place < attesters.length && (i == 0 || place > signers[i - 1]), "signers ascending, each once");
            require(recover(linked, signatures[i * 65:(i + 1) * 65]) == attesters[place], "an attestation is not its attester's");
            weight += weights[place];
        }
    }

    /// Takes a link that holds: its target justified, its source final when the target is the
    /// very next epoch, and the next epoch's set as the one that signs from now on.
    function record(bytes32[6] memory link, bytes32 nextSet) private {
        uint256 sourceEpoch = uint256(link[0]);
        uint256 targetEpoch = uint256(link[2]);
        justifiedEpoch = targetEpoch;
        justifiedBlock = link[3];
        blockAt[targetEpoch] = link[3];
        heightAt[targetEpoch] = uint256(link[4]);
        stateRootAt[targetEpoch] = link[5];
        emit Justified(targetEpoch, link[3], uint256(link[4]), link[5]);

        // Final only once its own link was taken here, so every final checkpoint has its root.
        if (targetEpoch == sourceEpoch + 1 && sourceEpoch > finalizedEpoch && blockAt[sourceEpoch] == link[1]) {
            finalizedEpoch = sourceEpoch;
            finalizedBlock = link[1];
            finalAt[sourceEpoch] = true;
            emit Finalized(sourceEpoch, link[1]);
        }

        if (nextSet != setHash) {
            setHash = nextSet;
            emit SetMoved(nextSet);
        }
    }

    /// How many verifiers it has, which verifiers(i) lists.
    function verifierCount() external view returns (uint256) {
        return verifiers.length;
    }

    /// Whether any verifier takes the proof under the program: each is asked in turn, as each of
    /// SP1's gateways refuses a proof of another kind.
    function verified(bytes32 by, bytes calldata publicValues, bytes calldata proof) internal view returns (bool) {
        for (uint256 i = 0; i < verifiers.length; i++) {
            try verifiers[i].verifyProof(by, publicValues, proof) {
                return true;
            } catch {}
        }
        return false;
    }

    /// Takes a validity proof of the range from the proven tip to the final checkpoint of an epoch,
    /// whose public values say so: eleven words, chainId, genesis, startHeight, startBlock,
    /// startRoot, endHeight, endBlock, endRoot, withdrawals, program, gatewayMode. The end must be
    /// the checkpoint this contract finalized, with its height and state root, so the proof shows
    /// that the state root the validators signed is the one the blocks come to.
    function prove(bytes calldata publicValues, bytes calldata proof, uint256 epoch) external {
        require(program != bytes32(0) && verifiers.length > 0, "no program to prove");
        require(publicValues.length == 352, "eleven words of public values");
        bytes32[11] memory v = abi.decode(publicValues, (bytes32[11]));
        require(uint256(v[0]) == chainId && v[1] == genesis, "a proof of another chain");
        require(uint256(v[2]) == provenHeight && v[3] == provenBlock && v[4] == provenRoot, "a range from elsewhere than the proven tip");
        require(finalAt[epoch] && blockAt[epoch] == v[6] && heightAt[epoch] == uint256(v[5]) && stateRootAt[epoch] == v[7], "a range that ends at no final checkpoint here");
        require(uint256(v[5]) > provenHeight, "a range that ends after it starts");
        require(uint256(v[10]) <= 2, "no such gateway mode");
        require(verified(program, publicValues, proof), "no verifier here takes the proof");

        provenHeight = uint256(v[5]);
        provenBlock = v[6];
        provenRoot = v[7];
        withdrawalsOf[v[6]] = v[8];
        provenEnds.push(provenHeight);
        provenBlockAt[provenHeight] = v[6];
        gatewayMode = uint8(uint256(v[10]));
        emit Proven(provenHeight, provenBlock, provenRoot, v[8]);

        if (v[9] != bytes32(0) && v[9] != program) {
            program = v[9];
            emit ProgramMoved(v[9]);
        }
    }

    /// How many ranges were proven here.
    function provenCount() external view returns (uint256) {
        return provenEnds.length;
    }

    /// Whether a transfer's digest left chainv4 in the proven range that ends at a block, by its
    /// proof in that range's withdrawals tree: each leaf keccak256 of a digest, each node keccak256
    /// of its two children, the lower first.
    function withdrew(bytes32 endBlock, bytes32 transfer, bytes32[] calldata path) external view returns (bool) {
        bytes32 root = withdrawalsOf[endBlock];
        bytes32 node = keccak256(abi.encode(transfer));

        for (uint256 i = 0; i < path.length; i++) {
            node = node <= path[i] ? keccak256(abi.encode(node, path[i])) : keccak256(abi.encode(path[i], node));
        }

        return root != bytes32(0) && node == root;
    }

    function recover(bytes32 linked, bytes calldata signature) private pure returns (address) {
        bytes32 r = bytes32(signature[0:32]);
        bytes32 s = bytes32(signature[32:64]);
        uint8 v = uint8(signature[64]);
        require(uint256(s) <= HALF_ORDER && (v == 27 || v == 28), "an attestation not in its one shape");
        address recovered = ecrecover(linked, v, r, s);
        require(recovered != address(0), "an attestation that recovers no key");

        return recovered;
    }
}
