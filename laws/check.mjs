#!/usr/bin/env node
// Gate for the Bend spec: the proofs must check, and extension/rules.ts must agree with the compiled rules.bend.
import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { draw, SCORES, SWITCHED } from "../extension/rules.ts";

const HERE = dirname(fileURLToPath(import.meta.url));
const INSTALLED = join(homedir(), ".bend", "bin", "bend");
const BEND = process.env.BEND ?? (existsSync(INSTALLED) ? INSTALLED : "bend");
const DEALS = 2000;
const OUTCOMES = { Failed: "failed", Fixes: "needed my fixes", Nailed: "nailed it" };

const failures = [];
const expect = (ok, what) => {
	if (!ok) failures.push(what);
};

function bend(args) {
	return execFileSync(BEND, args, { cwd: HERE, encoding: "utf8", env: { ...process.env, BEND_NO_TELEMETRY: "1" } });
}

const toList = (items) => items.reduceRight((tail, head) => ({ $: "Con", head, tail }), { $: "Nil" });

function fromList(list) {
	const items = [];
	for (let node = list; node.$ === "Con"; node = node.tail) items.push(node.head);
	return items;
}

function shuffled(items) {
	const output = [...items];
	for (let i = output.length - 1; i > 0; i--) {
		const j = Math.floor(Math.random() * (i + 1));
		[output[i], output[j]] = [output[j], output[i]];
	}
	return output;
}

const proofs = bend(["PROOF.bend"]);
expect(proofs.includes("ALL PROOFS CHECK"), `bend PROOF.bend did not print ALL PROOFS CHECK:\n${proofs}`);

const out = mkdtempSync(join(tmpdir(), "whodunit-laws-"));
try {
	bend(["rules.bend", "-o", join(out, "rules.mjs")]);
	const { default: spec } = await import(pathToFileURL(join(out, "rules.mjs")).href);

	for (const { score, outcome } of SCORES) {
		const folded = OUTCOMES[spec.fold(score).$];
		expect(folded === outcome, `score ${score}: rules.ts says ${outcome}, Bend says ${folded}`);
	}
	const switched = spec.finish({ $: "True" }, 4);
	expect(
		switched.score === SWITCHED.score && OUTCOMES[switched.outcome.$] === SWITCHED.outcome,
		`a switched run: rules.ts logs ${SWITCHED.score}/${SWITCHED.outcome}, Bend logs ${switched.score}/${OUTCOMES[switched.outcome.$]}`,
	);

	let deck = [];
	for (let n = 0; n < DEALS; n++) {
		const contenders = [0, 1, 2, 3].slice(0, 2 + (n % 3));
		const refill = shuffled(contenders);
		const legal = deck.filter((card) => contenders.includes(card));
		const ts = draw(legal, contenders, () => refill);
		const hand = spec.deal_n(1, toList(legal), toList(refill));
		const [card] = fromList(hand.dealt);
		const rest = fromList(hand.left);
		expect(card === ts.card && rest.join() === ts.rest.join(), `deal ${n} from [${legal}] refilling [${refill}]: rules.ts ${ts.card} [${ts.rest}], Bend ${card} [${rest}]`);
		deck = ts.rest;
	}
} finally {
	rmSync(out, { recursive: true, force: true });
}

if (failures.length > 0) {
	console.error(`${failures.length} mismatch(es):\n${failures.slice(0, 10).join("\n")}`);
	process.exit(1);
}
console.log(`ALL PROOFS CHECK, and rules.ts agrees with rules.bend on ${SCORES.length} scores, a switched run and ${DEALS} deals.`);
