import { closeSync, existsSync, mkdirSync, openSync, readFileSync, renameSync, unlinkSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";
import { setTimeout } from "node:timers/promises";
import { draw } from "./rules.ts";

/** Serialize the whole read/draw/write across the independent pi processes. */
export async function dealCard(path: string, role: string, contenders: string[], shuffle: () => string[]): Promise<string> {
	mkdirSync(dirname(path), { recursive: true });
	const lock = `${path}.lock`;
	const deadline = Date.now() + 10_000;
	let fd: number;
	while (true) {
		try {
			fd = openSync(lock, "wx");
			break;
		} catch (error) {
			if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
			if (Date.now() >= deadline) throw new Error(`whodunit: deck is locked. If no other chat is dealing, remove ${lock} and retry.`);
			await setTimeout(20);
		}
	}
	try {
		const decks: Record<string, string[]> = existsSync(path) ? JSON.parse(readFileSync(path, "utf8")) : {};
		const { card, rest } = draw(decks[role] ?? [], contenders, shuffle);
		decks[role] = rest;
		const tmp = `${path}.${process.pid}.tmp`;
		writeFileSync(tmp, `${JSON.stringify(decks, null, 2)}\n`);
		renameSync(tmp, path);
		return card;
	} finally {
		closeSync(fd);
		unlinkSync(lock);
	}
}
