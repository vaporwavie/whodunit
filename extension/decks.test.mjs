import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { dealCard } from "./decks.ts";

function child(path, role) {
	const code = `import { dealCard } from ${JSON.stringify(new URL("./decks.ts", import.meta.url).href)};
		const cards = ["a", "b", "c", "d"];
		console.log(await dealCard(process.argv[1], process.argv[2], cards, () => cards));`;
	return new Promise((resolve, reject) => {
		const process = spawn(globalThis.process.execPath, ["--input-type=module", "-e", code, path, role]);
		let stdout = "", stderr = "";
		process.stdout.on("data", (chunk) => { stdout += chunk; });
		process.stderr.on("data", (chunk) => { stderr += chunk; });
		process.on("error", reject);
		process.on("close", (code) => code === 0 ? resolve(stdout.trim()) : reject(new Error(stderr)));
	});
}

test("parallel pi processes consume each card once per deck without losing other roles", async () => {
	const dir = mkdtempSync(join(tmpdir(), "whodunit-decks-"));
	const path = join(dir, "decks.json");
	try {
		const draws = await Promise.all(Array.from({ length: 16 }, (_, i) => child(path, i % 2 ? "builder" : "brainiac")));
		for (const parity of [0, 1]) {
			const cards = draws.filter((_, i) => i % 2 === parity).sort();
			assert.deepEqual(cards, ["a", "a", "b", "b", "c", "c", "d", "d"]);
		}
		assert.deepEqual(JSON.parse(readFileSync(path, "utf8")), { builder: [], brainiac: [] });
		assert.equal(existsSync(`${path}.lock`), false);
	} finally {
		rmSync(dir, { recursive: true, force: true });
	}
});

test("a failed draw releases its lock and preserves the deck", async () => {
	const dir = mkdtempSync(join(tmpdir(), "whodunit-decks-"));
	const path = join(dir, "decks.json");
	writeFileSync(path, '{"builder":[]}');
	try {
		await assert.rejects(dealCard(path, "builder", ["a"], () => { throw new Error("shuffle failed"); }), /shuffle failed/);
		assert.equal(existsSync(`${path}.lock`), false);
		assert.equal(readFileSync(path, "utf8"), '{"builder":[]}');
		assert.equal(await dealCard(path, "builder", ["a"], () => ["a"]), "a");
	} finally {
		rmSync(dir, { recursive: true, force: true });
	}
});
