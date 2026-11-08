import { spawn, type ChildProcess } from "node:child_process";
import { appendFileSync, existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { basename, join } from "node:path";
import type { Message } from "@earendil-works/pi-ai";
import { getMarkdownTheme, type ExtensionAPI, type ExtensionContext } from "@earendil-works/pi-coding-agent";
import { Box, Markdown, Text, truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";

const PROVIDER = "agent-week";
const MODEL_ID = "auto";
const DIR = join(homedir(), ".pi", "agent", "agent-week");
const ROLES_PATH = join(DIR, "roles.json");
const DECKS_PATH = join(DIR, "decks.json");
const RUNS_PATH = join(DIR, "runs.jsonl");
const DEAL = "agent-week.deal";
const DONE = "agent-week.done";
const SWITCH = "agent-week.switch";
const FANOUT_VIEW = "agent-week.fanout-view";
const FANOUT_PICK = "agent-week.fanout-pick";
const FANOUT_TIMEOUT_MS = 20 * 60 * 1000;
const FANOUT_TOOLS = "read,grep,find,ls,mcp__exa__*";
const LABELS = ["A", "B", "C", "D"];

type Effort = "low" | "medium" | "high" | "xhigh" | "max";

interface RoleConfig {
	effort: Effort;
	criteria: string;
	contenders: string[];
	fanout?: boolean;
}

interface Config {
	classifier: { provider: string; model: string; minConfidence: number };
	agents: Record<string, string>;
	roles: Record<string, RoleConfig>;
}

interface Deal {
	role: string;
	agent: string;
	effort: Effort;
	size: string;
	confidence: number;
	probabilities: Record<string, number>;
	source: "jev" | "picker" | "forced" | "fallback";
	at: string;
}

interface FanoutEntry {
	label: string;
	agent: string;
	ok: boolean;
	error?: string;
	text: string;
	costUsd: number;
	seconds: number;
	toolCalls: Record<string, number>;
}

interface Classification {
	role: string;
	size: string;
	confidence: number;
	probabilities: Record<string, number>;
	source: Deal["source"];
}

function loadConfig(): Config {
	return JSON.parse(readFileSync(ROLES_PATH, "utf8")) as Config;
}

function splitAgent(agent: string): { provider: string; id: string } {
	const slash = agent.indexOf("/");
	return { provider: agent.slice(0, slash), id: agent.slice(slash + 1) };
}

function shuffle<T>(items: readonly T[]): T[] {
	const output = [...items];
	for (let i = output.length - 1; i > 0; i--) {
		const j = Math.floor(Math.random() * (i + 1));
		[output[i], output[j]] = [output[j], output[i]];
	}
	return output;
}

function dealCard(role: string, contenders: string[]): string {
	mkdirSync(DIR, { recursive: true });
	const decks: Record<string, string[]> = existsSync(DECKS_PATH) ? JSON.parse(readFileSync(DECKS_PATH, "utf8")) : {};
	const remaining = (decks[role] ?? []).filter((agent) => contenders.includes(agent));
	const deck = remaining.length > 0 ? remaining : shuffle(contenders);
	const [card, ...rest] = deck;
	decks[role] = rest;
	const tmp = `${DECKS_PATH}.${process.pid}.tmp`;
	writeFileSync(tmp, `${JSON.stringify(decks, null, 2)}\n`);
	renameSync(tmp, DECKS_PATH);
	return card;
}

function appendRun(record: Record<string, unknown>): void {
	mkdirSync(DIR, { recursive: true });
	appendFileSync(RUNS_PATH, `${JSON.stringify({ ...record, at: new Date().toISOString() })}\n`);
}

function findEntry<T>(ctx: ExtensionContext, customType: string): T | undefined {
	const entry = ctx.sessionManager
		.getBranch()
		.findLast((candidate) => candidate.type === "custom" && candidate.customType === customType);
	if (!entry || entry.type !== "custom") return undefined;
	return entry.data as T;
}

function isAgentWeek(model: { provider: string; id: string } | undefined): boolean {
	return model?.provider === PROVIDER && model.id === MODEL_ID;
}

function lastUserText(messages: readonly Message[]): string {
	const content = messages.filter((message) => message.role === "user").at(-1)?.content ?? "";
	if (typeof content === "string") return content;
	return content.flatMap((block) => (block.type === "text" ? [block.text] : [])).join("\n");
}

function sessionRef(ctx: ExtensionContext): { sessionId: string; sessionFile: string | undefined } {
	return { sessionId: ctx.sessionManager.getSessionId(), sessionFile: ctx.sessionManager.getSessionFile() };
}

async function classify(ctx: ExtensionContext, config: Config, prompt: string): Promise<Classification | undefined> {
	const jev = ctx.modelRegistry.findOfType("classifier", config.classifier.provider, config.classifier.model);
	if (!jev) return undefined;
	const roleCriteria = Object.fromEntries(Object.entries(config.roles).map(([name, role]) => [name, role.criteria]));
	const result = await ctx.modelRegistry.classify(jev, {
		state: { prompt: prompt.slice(0, 16_000) },
		questions: {
			role: {
				type: "choice",
				instructions: "Which kind of agent should handle the request in `prompt`?",
				criteria: roleCriteria,
			},
			size: {
				type: "choice",
				instructions: "How much work does the request in `prompt` take for a capable coding agent?",
				criteria: { S: "Minutes", M: "Under an hour", L: "Hours or more" },
			},
		},
	});
	if (result.stopReason !== "stop") return undefined;
	const roleAnswer = result.answers.role;
	const sizeAnswer = result.answers.size;
	if (roleAnswer?.type !== "choice") return undefined;
	const { choice: role, probabilities } = roleAnswer;
	const confidence = probabilities[role] ?? 0;
	const size = sizeAnswer?.type === "choice" ? sizeAnswer.choice : "?";
	return { role, size, confidence, probabilities, source: "jev" };
}

async function confirmRole(ctx: ExtensionContext, config: Config, classification: Classification): Promise<Classification> {
	if (classification.confidence >= config.classifier.minConfidence || !ctx.hasUI) return classification;
	const suggested = `${classification.role} (suggested, ${Math.round(classification.confidence * 100)}%)`;
	const others = Object.keys(config.roles).filter((name) => name !== classification.role);
	const choice = await ctx.ui.select("Jev isn't sure. Which role fits?", [suggested, ...others]);
	if (!choice || choice === suggested) return classification;
	return { ...classification, role: choice, source: "picker" };
}

function textOf(message: { content?: unknown }): string {
	if (!Array.isArray(message.content)) return "";
	return message.content
		.flatMap((block: { type?: string; text?: string }) => (block.type === "text" && block.text ? [block.text] : []))
		.join("");
}

function runChild(
	agent: string,
	effort: Effort,
	prompt: string,
	cwd: string,
	children: Set<ChildProcess>,
): Promise<Omit<FanoutEntry, "label">> {
	const started = Date.now();
	return new Promise((resolve) => {
		const args = [
			"-p",
			"--no-session",
			"-ne",
			"-e",
			"builtin:mcp",
			"--mode",
			"json",
			"--tools",
			FANOUT_TOOLS,
			"--model",
			`${agent}:${effort}`,
			prompt,
		];
		const child = spawn("pi", args, { cwd, env: { ...process.env, AGENT_WEEK_CHILD: "1" }, stdio: ["ignore", "pipe", "pipe"] });
		children.add(child);
		let buffer = "";
		let text = "";
		let costUsd = 0;
		const toolCalls: Record<string, number> = {};
		let error: string | undefined;
		let stderr = "";
		const timer = setTimeout(() => child.kill("SIGTERM"), FANOUT_TIMEOUT_MS);
		const handleLine = (line: string) => {
			if (!line.trim()) return;
			let event: { type?: string; toolName?: string; message?: { role?: string; usage?: { cost?: { total?: number } }; stopReason?: string; errorMessage?: string; content?: unknown } };
			try {
				event = JSON.parse(line);
			} catch {
				return;
			}
			if (event.type === "tool_execution_start" && event.toolName) {
				toolCalls[event.toolName] = (toolCalls[event.toolName] ?? 0) + 1;
				return;
			}
			if (event.type !== "message_end" || event.message?.role !== "assistant") return;
			costUsd += event.message.usage?.cost?.total ?? 0;
			const messageText = textOf(event.message);
			if (messageText) text = messageText;
			if (event.message.stopReason === "error") error = event.message.errorMessage ?? "error";
		};
		child.stdout?.on("data", (chunk: Buffer) => {
			buffer += chunk.toString();
			const lines = buffer.split("\n");
			buffer = lines.pop() ?? "";
			lines.forEach(handleLine);
		});
		child.stderr?.on("data", (chunk: Buffer) => {
			stderr = (stderr + chunk.toString()).slice(-2000);
		});
		child.on("close", (code) => {
			clearTimeout(timer);
			children.delete(child);
			handleLine(buffer);
			const seconds = Math.round((Date.now() - started) / 1000);
			const failed = code !== 0 || !text;
			const reason = error ?? (failed ? `exit ${code}: ${stderr.trim().split("\n").at(-1) ?? ""}` : undefined);
			resolve({ agent, ok: !failed, error: reason, text, costUsd, seconds, toolCalls });
		});
	});
}

async function fanOut(
	pi: ExtensionAPI,
	ctx: ExtensionContext,
	config: Config,
	classification: Classification,
	prompt: string,
	children: Set<ChildProcess>,
): Promise<Deal> {
	const role = config.roles[classification.role];
	const order = shuffle(role.contenders);
	const started = Date.now();
	const status: string[] = order.map(() => "running");
	const render = () => {
		const elapsed = Math.round((Date.now() - started) / 1000);
		const parts = order.map((_agent, index) => `${LABELS[index]} ${status[index]}`);
		ctx.ui.setWidget(PROVIDER, [`${classification.role} fan-out · ${parts.join(" · ")} · ${elapsed}s`]);
	};
	render();
	const ticker = setInterval(render, 1000);
	const results = await Promise.all(
		order.map(async (agent, index) => {
			const result = await runChild(agent, role.effort, prompt, ctx.cwd, children);
			status[index] = result.ok ? "done" : "failed";
			render();
			return { ...result, label: LABELS[index] };
		}),
	);
	clearInterval(ticker);
	ctx.ui.setWidget(PROVIDER, undefined);

	pi.appendEntry(FANOUT_VIEW, {
		role: classification.role,
		prompt,
		answers: results.map(({ label, ok, error, text, toolCalls }) => ({ label, ok, error, text, searched: Object.keys(toolCalls).some((name) => name.startsWith("mcp__exa__")) })),
	});
	const pickable = results.filter((result) => result.ok);
	const options = pickable.length > 0 ? pickable.map((result) => result.label) : results.map((result) => result.label);
	let picked = await ctx.ui.select("Which answer is best?", options);
	while (!picked) picked = await ctx.ui.select("Pick one to continue with (it becomes this session's agent)", options);
	const winner = results.find((result) => result.label === picked) ?? results[0];

	appendRun({
		type: "fanout",
		...sessionRef(ctx),
		role: classification.role,
		winner: winner.agent,
		entries: results.map(({ label, agent, ok, error, costUsd, seconds, toolCalls }) => ({ label, agent, ok, error, costUsd, seconds, toolCalls })),
	});
	pi.sendMessage(
		{
			customType: FANOUT_PICK,
			content: `User request:\n\n${prompt}\n\nYour answer:\n\n${winner.text}`,
			display: false,
			details: { label: winner.label },
		},
		{ triggerTurn: false },
	);
	pi.appendEntry(FANOUT_PICK, { label: winner.label, text: winner.text });
	return {
		role: classification.role,
		agent: winner.agent,
		effort: role.effort,
		size: classification.size,
		confidence: classification.confidence,
		probabilities: classification.probabilities,
		source: classification.source,
		at: new Date().toISOString(),
	};
}

function setFooter(ctx: ExtensionContext, label: () => string): void {
	if (!ctx.hasUI) return;
	ctx.ui.setFooter((tui, theme, footerData) => {
		const unsubscribe = footerData.onBranchChange(() => tui.requestRender());
		return {
			dispose: unsubscribe,
			invalidate() {},
			render(width: number): string[] {
				const left = theme.fg("accent", label());
				const branch = footerData.getGitBranch();
				const where = branch ? `${basename(ctx.cwd)} (${branch})` : basename(ctx.cwd);
				const right = theme.fg("dim", where);
				const pad = " ".repeat(Math.max(1, width - visibleWidth(left) - visibleWidth(right)));
				return [truncateToWidth(left + pad + right, width)];
			},
		};
	});
}

export default function (pi: ExtensionAPI) {
	if (process.env.AGENT_WEEK_CHILD) return;

	const children = new Set<ChildProcess>();
	let forcedRole: string | undefined;

	const footerLabel = (ctx: ExtensionContext) => () => {
		const deal = findEntry<Deal>(ctx, DEAL);
		const done = findEntry<{ outcome: string }>(ctx, DONE);
		if (!deal) return forcedRole ? `agent-week · ${forcedRole} (forced) · waiting for first prompt` : "agent-week · waiting for first prompt";
		const suffix = done ? " · rated" : " · /done when finished";
		return `agent-week · ${deal.role} · ${deal.size}${suffix}`;
	};

	const recordDeal = (ctx: ExtensionContext, deal: Deal) => {
		pi.appendEntry(DEAL, deal);
		appendRun({ type: "deal", ...sessionRef(ctx), role: deal.role, agent: deal.agent, size: deal.size, source: deal.source, confidence: deal.confidence });
	};

	const cardDeal = (config: Config, classification: Classification): Deal => {
		const role = config.roles[classification.role];
		return {
			role: classification.role,
			agent: dealCard(classification.role, role.contenders),
			effort: role.effort,
			size: classification.size,
			confidence: classification.confidence,
			probabilities: classification.probabilities,
			source: classification.source,
			at: new Date().toISOString(),
		};
	};

	const pickClassification = async (ctx: ExtensionContext, config: Config, prompt: string): Promise<Classification> => {
		if (forcedRole) {
			const role = forcedRole;
			forcedRole = undefined;
			return { role, size: "?", confidence: 1, probabilities: { [role]: 1 }, source: "forced" };
		}
		const classification = await classify(ctx, config, prompt).catch(() => undefined);
		if (!classification) return { role: "builder", size: "?", confidence: 0, probabilities: {}, source: "fallback" };
		return confirmRole(ctx, config, classification);
	};

	pi.registerVirtualModel({
		provider: PROVIDER,
		id: MODEL_ID,
		name: "Agent week",
		thinkingLevels: ["medium"],
		contextWindow: 272_000,
		maxTokens: 128_000,
		async route(request, ctx) {
			const config = loadConfig();
			let deal = findEntry<Deal>(ctx, DEAL);
			if (!deal && request.reason !== "direct") {
				const classification = await pickClassification(ctx, config, lastUserText(request.messages));
				deal = cardDeal(config, classification);
				recordDeal(ctx, deal);
			}
			const agent = deal?.agent ?? "anthropic/claude-opus-5-5";
			const effort = deal?.effort ?? "medium";
			const { provider, id } = splitAgent(agent);
			const model = ctx.modelRegistry.find(provider, id);
			if (!model) throw new Error(`agent-week: ${agent} is not in the catalog`);
			return { model, thinkingLevel: effort };
		},
	});

	pi.on("input", async (event, ctx) => {
		if (event.source === "extension") return { action: "continue" };
		if (!isAgentWeek(ctx.model)) return { action: "continue" };
		if (findEntry<Deal>(ctx, DEAL)) return { action: "continue" };

		const config = loadConfig();
		const classification = await pickClassification(ctx, config, event.text);
		const role = config.roles[classification.role];
		const fanoutReady = role?.fanout && ctx.hasUI && role.contenders.length <= LABELS.length;
		if (!fanoutReady) {
			recordDeal(ctx, cardDeal(config, classification));
			setFooter(ctx, footerLabel(ctx));
			return { action: "continue" };
		}
		const deal = await fanOut(pi, ctx, config, classification, event.text, children);
		recordDeal(ctx, deal);
		setFooter(ctx, footerLabel(ctx));
		return { action: "handled" };
	});

	pi.on("session_start", async (_event, ctx) => {
		forcedRole = undefined;
		if (!isAgentWeek(ctx.model)) return;
		setFooter(ctx, footerLabel(ctx));
		if (!process.env.EXA_API_KEY && ctx.hasUI) ctx.ui.notify("EXA_API_KEY isn't set, so scout fan-outs run without web search.", "warning");
	});

	pi.on("model_select", async (event, ctx) => {
		if (isAgentWeek(event.model)) {
			setFooter(ctx, footerLabel(ctx));
			return;
		}
		if (findEntry<Deal>(ctx, DEAL) && !findEntry(ctx, SWITCH)) {
			pi.appendEntry(SWITCH, { to: `${event.model?.provider}/${event.model?.id}`, at: new Date().toISOString() });
		}
		if (ctx.hasUI) ctx.ui.setFooter(undefined);
	});

	pi.on("session_shutdown", async () => {
		for (const child of children) child.kill("SIGTERM");
	});

	pi.registerEntryRenderer(FANOUT_VIEW, (entry, _options, theme) => {
		const data = entry.data as { role: string; prompt: string; answers: { label: string; ok: boolean; error?: string; text: string; searched?: boolean }[] };
		const box = new Box(1, 1, (text) => theme.bg("customMessageBg", text));
		box.addChild(new Text(theme.fg("dim", data.prompt)));
		box.addChild(new Text(""));
		box.addChild(new Text(theme.bold(`${data.role} fan-out: pick the best answer`)));
		for (const answer of data.answers) {
			box.addChild(new Text(""));
			box.addChild(new Text(theme.fg("accent", theme.bold(`── ${answer.label} ──`))));
			if (!answer.ok) box.addChild(new Text(theme.fg("error", `failed: ${answer.error ?? "no answer"}`)));
			if (data.role === "scout" && answer.searched === false) box.addChild(new Text(theme.fg("warning", "(no web search used)")));
			if (answer.text) box.addChild(new Markdown(answer.text, 0, 0, getMarkdownTheme()));
		}
		return box;
	});

	pi.registerEntryRenderer(FANOUT_PICK, (entry, _options, theme) => {
		const data = entry.data as { label: string; text: string };
		const box = new Box(1, 1, (text) => theme.bg("customMessageBg", text));
		box.addChild(new Text(theme.fg("accent", theme.bold(`Picked ${data.label}. Follow-ups go to the same agent.`))));
		return box;
	});

	pi.registerCommand("role", {
		description: "Show the role, or force one before the first prompt: /role <name>",
		getArgumentCompletions: (prefix: string) => {
			const names = Object.keys(loadConfig().roles).filter((name) => name.startsWith(prefix));
			return names.length > 0 ? names.map((name) => ({ value: name, label: name })) : null;
		},
		handler: async (args, ctx) => {
			const config = loadConfig();
			const deal = findEntry<Deal>(ctx, DEAL);
			const requested = args.trim();
			if (!requested) {
				const current = deal?.role ?? forcedRole ?? "not chosen yet";
				ctx.ui.notify(`Role: ${current}`, "info");
				return;
			}
			if (deal) {
				ctx.ui.notify(`The role is already ${deal.role} for this session. Use /new for a new task.`, "warning");
				return;
			}
			if (!config.roles[requested]) {
				ctx.ui.notify(`Unknown role. Roles: ${Object.keys(config.roles).join(", ")}`, "error");
				return;
			}
			forcedRole = requested;
			setFooter(ctx, footerLabel(ctx));
			ctx.ui.notify(`Next prompt goes to ${requested}`, "info");
		},
	});

	pi.registerCommand("done", {
		description: "Rate this run, then reveal which model did it",
		handler: async (_args, ctx) => {
			const config = loadConfig();
			const deal = findEntry<Deal>(ctx, DEAL);
			if (!deal) {
				ctx.ui.notify("No agent-week run in this session yet.", "warning");
				return;
			}
			const previous = findEntry<{ outcome: string; score: number }>(ctx, DONE);
			if (previous) {
				ctx.ui.notify(`Already rated: ${previous.outcome}, ${previous.score}/5. It was ${config.agents[deal.agent] ?? deal.agent}.`, "info");
				return;
			}
			const switched = findEntry<{ to: string }>(ctx, SWITCH);
			const outcomes = ["nailed it", "needed my fixes", "failed"];
			const chosenOutcome = switched ? "failed" : await ctx.ui.select("How did it go?", outcomes);
			if (!chosenOutcome) return;
			if (switched) ctx.ui.notify(`You switched to ${switched.to} mid-run, so this run counts as failed.`, "warning");
			const scoreChoice = await ctx.ui.select("Score, 1 (bad) to 5 (great)", ["5", "4", "3", "2", "1"]);
			if (!scoreChoice) return;
			const score = Number(scoreChoice);
			const record = { outcome: chosenOutcome, score, switched: Boolean(switched) };
			pi.appendEntry(DONE, { ...record, at: new Date().toISOString() });
			appendRun({ type: "done", ...sessionRef(ctx), role: deal.role, agent: deal.agent, ...record });
			setFooter(ctx, footerLabel(ctx));
			ctx.ui.notify(`It was ${config.agents[deal.agent] ?? deal.agent}. Logged ${chosenOutcome}, ${score}/5.`, "info");
		},
	});
}
