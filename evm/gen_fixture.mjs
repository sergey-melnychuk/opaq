// Convert a real Groth16 proof (snarkjs proof.json + public.json) into a
// Solidity fixture a Foundry test loads. Uses `snarkjs zkey export
// soliditycalldata`, which emits the args already in EVM order (it handles
// the G2 coordinate swap for the pairing). The public-signal count is read
// from public.json itself (burn = 6, xburn = 4, B.12.2), so this works for
// any circuit unmodified.
// Usage: node gen_fixture.mjs <public.json> <proof.json> <out.sol> [libName]
import { execFileSync } from "node:child_process";
import fs from "node:fs";

const [pub, proof, out, libName] = process.argv.slice(2);
if (!pub || !proof || !out) {
  console.error("usage: node gen_fixture.mjs <public.json> <proof.json> <out.sol> [libName]");
  process.exit(2);
}
const lib = libName || "BurnProof";

const calldata = execFileSync("snarkjs", ["zkey", "export", "soliditycalldata", pub, proof], { encoding: "utf8" }).trim();
const [a, b, c, sig] = JSON.parse("[" + calldata + "]");

const sol = `// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;
// @generated from a real proof by evm/gen_fixture.mjs — do not edit.
library ${lib} {
    function load()
        internal
        pure
        returns (uint[2] memory a, uint[2][2] memory b, uint[2] memory c, uint[${sig.length}] memory sig)
    {
        a[0] = ${a[0]}; a[1] = ${a[1]};
        b[0][0] = ${b[0][0]}; b[0][1] = ${b[0][1]};
        b[1][0] = ${b[1][0]}; b[1][1] = ${b[1][1]};
        c[0] = ${c[0]}; c[1] = ${c[1]};
${sig.map((s, i) => `        sig[${i}] = ${s};`).join("\n")}
    }
}
`;
fs.writeFileSync(out, sol);
console.log(`wrote ${out}`);
