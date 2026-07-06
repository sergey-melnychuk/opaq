// Self-served cross-chain MINT (OPAQ.md A.6, EVM side): given a real proof
// (snarkjs public.json + proof.json), format it as `<fn>(a,b,c,signals)`
// calldata and submit the tx — NO relayer, the note owner acts for themselves.
// Uses snarkjs (`export soliditycalldata`, which already orders the G2 coords
// for the pairing) + Foundry `cast` for the transaction. Defaults to
// OpaqMint's original `mint(...,uint256[6])` (m15/m17/m18); pass a different
// function signature + signal count for OpaqPool's `mintFromXburn`/`xburn`
// (both take uint256[4], B.12.2/P4.3's m20).
//
// Usage: node mint.mjs <rpc> <contractAddr> <privKey> <public.json> <proof.json> [fnSig] [nSignals]
import { execFileSync } from "node:child_process";

const [rpc, addr, key, pub, proof, fnSigArg, nSignalsArg] = process.argv.slice(2);
if (!rpc || !addr || !key || !pub || !proof) {
  console.error("usage: node mint.mjs <rpc> <contractAddr> <privKey> <public.json> <proof.json> [fnSig] [nSignals]");
  process.exit(2);
}
const nSignals = nSignalsArg || "6";
const fnSig = fnSigArg || `mint(uint256[2],uint256[2][2],uint256[2],uint256[${nSignals}])`;

// cast wants array literals as `[e0,e1]` (nested for uint[2][2]); the snarkjs
// calldata elements are already 0x-hex, so just bracket-join them.
const arr = (a) => `[${a.join(",")}]`;
const arr2 = (b) => `[${b.map(arr).join(",")}]`;

try {
  const calldata = execFileSync(
    "snarkjs", ["zkey", "export", "soliditycalldata", pub, proof], { encoding: "utf8" },
  ).trim();
  const [a, b, c, sig] = JSON.parse("[" + calldata + "]");
  const out = execFileSync("cast", [
    "send", addr, fnSig,
    arr(a), arr2(b), arr(c), arr(sig),
    "--rpc-url", rpc, "--private-key", key, "--json",
  ], { encoding: "utf8" });
  process.stdout.write((JSON.parse(out).transactionHash) || out.trim());
} catch (e) {
  const msg = e?.stderr?.toString?.() || e?.stdout?.toString?.() || e?.message || String(e);
  console.error(`mint: ${msg}`);
  process.exit(1);
}
