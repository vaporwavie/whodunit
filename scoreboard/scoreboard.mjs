#!/usr/bin/env node
import { existsSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

const HOME = homedir();
const DIR = join(HOME, ".pi", "agent", "whodunit");
const ROLES_PATH = join(DIR, "roles.json");
const RUNS_PATH = join(DIR, "runs.jsonl");
const IDLE_CAP_MIN = 10;
const TOO_FEW = 3;

function parseArgs(argv) {
	const args = { since: undefined, until: undefined };
	for (let i = 0; i < argv.length; i++) {
		if (argv[i] === "--since") args.since = new Date(argv[++i]);
		else if (argv[i] === "--until") args.until = new Date(argv[++i]);
	}
	return args;
}

function readJsonl(path) {
	if (!existsSync(path)) return [];
	return readFileSync(path, "utf8")
		.split("\n")
		.filter((line) => line.trim())
		.flatMap((line) => {
			try {
				return [JSON.parse(line)];
			} catch {
				return [];
			}
		});
}

const median = (values) => {
	if (values.length === 0) return undefined;
	const sorted = [...values].sort((a, b) => a - b);
	const mid = Math.floor(sorted.length / 2);
	return sorted.length % 2 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2;
};
const mean = (values) => (values.length === 0 ? undefined : values.reduce((sum, value) => sum + value, 0) / values.length);
const minutes = (ms) => ms / 60_000;
const fmt = (value, digits = 1) => (value === undefined || Number.isNaN(value) ? "–" : value.toFixed(digits));
const pct = (part, whole) => (whole === 0 ? "–" : `${Math.round((part / whole) * 100)}%`);

function isUserPrompt(message) {
	return message.role === "user";
}

function turnTiming(events) {
	let yourMs = 0;
	let agentMs = 0;
	let turnStart;
	let lastAgentEnd;
	for (const { kind, at } of events) {
		if (kind === "prompt") {
			if (lastAgentEnd !== undefined) yourMs += Math.min(at - lastAgentEnd, IDLE_CAP_MIN * 60_000);
			if (turnStart !== undefined && lastAgentEnd !== undefined && lastAgentEnd > turnStart) agentMs += lastAgentEnd - turnStart;
			turnStart = at;
			lastAgentEnd = undefined;
		} else if (kind === "agent") {
			lastAgentEnd = at;
		}
	}
	if (turnStart !== undefined && lastAgentEnd !== undefined && lastAgentEnd > turnStart) agentMs += lastAgentEnd - turnStart;
	return { yourMin: minutes(yourMs), agentMin: minutes(agentMs) };
}

function piRun(sessionFile, fanout) {
	const entries = readJsonl(sessionFile);
	const custom = (type) => entries.filter((entry) => entry.type === "custom" && entry.customType === type).map((entry) => entry.data);
	const [deal] = custom("whodunit.deal");
	if (!deal) return undefined;
	const [done] = custom("whodunit.done");
	const switched = custom("whodunit.switch").length > 0;
	const events = [];
	let costUsd = 0;
	let userPrompts = 0;
	let compactions = 0;
	for (const entry of entries) {
		const at = Date.parse(entry.timestamp);
		if (entry.type === "message" && isUserPrompt(entry.message)) {
			userPrompts += 1;
			events.push({ kind: "prompt", at });
		} else if (entry.type === "custom_message" && entry.customType === "whodunit.fanout-pick") {
			events.push({ kind: "prompt", at });
			events.push({ kind: "agent", at });
		} else if (entry.type === "message" && entry.message.role === "assistant") {
			costUsd += entry.message.usage?.cost?.total ?? 0;
			events.push({ kind: "agent", at });
		} else if (entry.type === "usage" || (entry.type === "compaction" && entry.usage)) {
			costUsd += entry.usage?.cost?.total ?? 0;
		}
		if (entry.type === "compaction") compactions += 1;
	}
	const timing = turnTiming(events.sort((a, b) => a.at - b.at));
	const winnerEntry = fanout?.entries.find((entry) => entry.agent === fanout.winner);
	const fanoutAgentMin = winnerEntry ? winnerEntry.seconds / 60 : 0;
	return {
		sessionFile,
		at: Date.parse(deal.at),
		role: deal.role,
		agent: deal.agent,
		source: deal.source,
		outcome: switched ? "failed" : done?.outcome,
		score: done?.score,
		switched,
		yourMin: timing.yourMin,
		agentMin: timing.agentMin + fanoutAgentMin,
		turnsAfterFirst: fanout ? userPrompts : Math.max(0, userPrompts - 1),
		costUsd: costUsd + (winnerEntry?.costUsd ?? 0),
		fanoutTotalUsd: fanout ? fanout.entries.reduce((sum, entry) => sum + entry.costUsd, 0) : 0,
		compactions,
	};
}

function collectPiRuns(args) {
	const runs = readJsonl(RUNS_PATH);
	const fanouts = new Map(runs.filter((run) => run.type === "fanout").map((run) => [run.sessionId, run]));
	const sessionFiles = new Map(runs.filter((run) => run.type === "deal" && run.sessionFile).map((run) => [run.sessionId, run.sessionFile]));
	const inWindow = (at) => (!args.since || at >= args.since.getTime()) && (!args.until || at < args.until.getTime());
	const results = [];
	for (const [sessionId, sessionFile] of sessionFiles) {
		if (!existsSync(sessionFile)) continue;
		const run = piRun(sessionFile, fanouts.get(sessionId));
		if (run && inWindow(run.at)) results.push(run);
	}
	const fanoutRecords = [...fanouts.values()].filter((run) => inWindow(Date.parse(run.at)));
	return { runs: results, fanoutRecords };
}

const DECIDERS = [
	["jev", "Jev"],
	["picker", "picker"],
	["forced", "/role"],
	["fallback", "fallback"],
];

function decisionLine(runs) {
	if (runs.length === 0) return undefined;
	const known = new Set(DECIDERS.map(([source]) => source));
	const count = (source) => runs.filter((run) => (known.has(run.source) ? run.source : "fallback") === source).length;
	return DECIDERS.map(([source, label]) => `${label} ${pct(count(source), runs.length)}`).join(" · ");
}

function rankRows(roleName, role, runs, fanoutRecords, agentNames) {
	const isFanout = Boolean(role.fanout);
	const rows = role.contenders.map((agent) => {
		const mine = runs.filter((run) => run.role === roleName && run.agent === agent);
		const rated = mine.filter((run) => run.outcome);
		const scored = rated.filter((run) => typeof run.score === "number");
		const entered = fanoutRecords.filter((record) => record.role === roleName && record.entries.some((entry) => entry.agent === agent));
		const wins = entered.filter((record) => record.winner === agent).length;
		const countForTooFew = isFanout ? entered.length : rated.length;
		return {
			agent: agentNames[agent] ?? agent,
			runs: mine.length,
			unrated: mine.length - rated.length,
			nailed: pct(rated.filter((run) => run.outcome === "nailed it").length, rated.length),
			meanScore: mean(scored.map((run) => run.score)),
			medianYourMin: median(mine.map((run) => run.yourMin)),
			costPerRun: isFanout
				? mean(entered.map((record) => record.entries.find((entry) => entry.agent === agent)?.costUsd ?? 0))
				: mean(mine.map((run) => run.costUsd)),
			wins,
			entered: entered.length,
			winShare: entered.length === 0 ? undefined : wins / entered.length,
			tooFew: countForTooFew < TOO_FEW,
		};
	});
	const rankKey = (row) => rankValue(isFanout, row) ?? -1;
	rows.sort((a, b) => (a.tooFew === b.tooFew ? rankKey(b) - rankKey(a) || (a.costPerRun ?? Infinity) - (b.costPerRun ?? Infinity) : a.tooFew ? 1 : -1));
	return rows;
}

const rankValue = (isFanout, row) => (isFanout ? row.winShare : row.meanScore);

// The agent winning a role: the top row with enough runs and something to rank by, and how far
// it stands ahead of the next one.
function leader(isFanout, rows) {
	const ranked = rows.filter((row) => !row.tooFew && rankValue(isFanout, row) !== undefined);
	if (ranked.length === 0) return undefined;
	const [top, next] = ranked;
	const figure = isFanout ? `${top.wins}/${top.entered} fan-outs won` : `${fmt(top.meanScore, 2)} mean score`;
	if (!next) return { row: top, figure, margin: "the only agent ranked so far" };
	const by = rankValue(isFanout, top) - rankValue(isFanout, next);
	if (by < 1e-9) {
		const cheaper = (top.costPerRun ?? Infinity) < (next.costPerRun ?? Infinity);
		return { row: top, figure, margin: `level with ${next.agent}${cheaper ? ", and cheaper" : ""}` };
	}
	const gap = isFanout ? `${fmt(by * 100, 0)} pts` : fmt(by, 2);
	return { row: top, figure, margin: `ahead of ${next.agent} by ${gap}` };
}

function roleTable(roleName, role, rows, runs) {
	const isFanout = Boolean(role.fanout);
	const lead = leader(isFanout, rows);
	const lines = [`### ${roleName}${isFanout ? " (ranked by fan-out win share)" : " (ranked by mean score)"}`, ""];
	const decided = decisionLine(runs.filter((run) => run.role === roleName));
	if (decided) lines.push(`Role decided by: ${decided}.`, "");
	lines.push("| agent | runs | nailed % | mean score | median your-min | cost/run | fan-out wins |");
	lines.push("|---|---|---|---|---|---|---|");
	for (const row of rows) {
		const runs = row.unrated > 0 ? `${row.runs} (${row.unrated} unrated)` : `${row.runs}`;
		const wins = isFanout ? `${row.wins}/${row.entered}` : "–";
		const name = row.tooFew ? `${row.agent} *too few*` : row === lead?.row ? `**${row.agent}** *(leads)*` : row.agent;
		lines.push(`| ${name} | ${runs} | ${row.nailed} | ${fmt(row.meanScore, 2)} | ${fmt(row.medianYourMin)} | $${fmt(row.costPerRun, 3)} | ${wins} |`);
	}
	return lines.join("\n");
}

function main() {
	const args = parseArgs(process.argv.slice(2));
	const config = JSON.parse(readFileSync(ROLES_PATH, "utf8"));
	const { runs, fanoutRecords } = collectPiRuns(args);
	const out = ["# Whodunit scoreboard", ""];
	const window = [args.since && `from ${args.since.toISOString().slice(0, 10)}`, args.until && `until ${args.until.toISOString().slice(0, 10)}`].filter(Boolean).join(" ");
	out.push(`${runs.length} runs${window ? ` ${window}` : ""}. Rows with fewer than ${TOO_FEW} rated runs (fan-outs for scout and scribe) are *too few* and can't win. No significance claims.`, "");
	const decided = decisionLine(runs);
	if (decided) out.push(`Role decided by: ${decided}.`, "");
	const tables = Object.entries(config.roles).map(([roleName, role]) => [roleName, role, rankRows(roleName, role, runs, fanoutRecords, config.agents)]);
	out.push("### Leaders", "", "| role | leader | ranked by | lead |", "|---|---|---|---|");
	for (const [roleName, role, rows] of tables) {
		const lead = leader(Boolean(role.fanout), rows);
		out.push(lead ? `| ${roleName} | **${lead.row.agent}** | ${lead.figure} | ${lead.margin} |` : `| ${roleName} | – | – | no agent has ${TOO_FEW} runs yet |`);
	}
	out.push("");
	for (const [roleName, role, rows] of tables) {
		out.push(roleTable(roleName, role, rows, runs), "");
	}
	const losersSpend = fanoutRecords
		.flatMap((record) => record.entries.filter((entry) => entry.agent !== record.winner))
		.reduce((sum, entry) => sum + entry.costUsd, 0);
	const runSpend = runs.reduce((sum, run) => sum + run.costUsd, 0);
	const switches = runs.filter((run) => run.switched).length;
	const compactions = runs.reduce((sum, run) => sum + run.compactions, 0);
	out.push(`Spend at list prices: $${fmt(runSpend + losersSpend, 2)} total, of which fan-out losers $${fmt(losersSpend, 2)}. Manual switches: ${switches}. Compactions: ${compactions}.`, "");
	console.log(out.join("\n"));
}

main();
