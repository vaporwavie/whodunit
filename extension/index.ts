import { spawn, type ChildProcess } from "node:child_process";
import { appendFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { basename, join } from "node:path";
import type { ImageContent, Message, TextContent } from "@earendil-works/pi-ai";
import { getMarkdownTheme, type ExtensionAPI, type ExtensionContext } from "@earendil-works/pi-coding-agent";
import { Box, Markdown, Text, truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";
import { type Ending, SCORES, SWITCHED } from "./rules.ts";
import { dealCard } from "./decks.ts";

const PROVIDER = "whodunit";
const MODEL_ID = "auto";
const DIR = join(homedir(), ".pi", "agent", "whodunit");
const ROLES_PATH = join(DIR, "roles.json");
const DECKS_PATH = join(DIR, "decks.json");
const RUNS_PATH = join(DIR, "runs.jsonl");
const DEAL = "whodunit.deal";
const DONE = "whodunit.done";
const SWITCH = "whodunit.switch";
const FANOUT_VIEW = "whodunit.fanout-view";
const FANOUT_PICK = "whodunit.fanout-pick";
const FANOUT_TIMEOUT_MS = 20 * 60 * 1000;
const FANOUT_TOOLS = "read,grep,find,ls,mcp__exa__*";
const LABELS = ["A", "B", "C", "D"];
const END_TITLE = "End this run";
const TITLER = "anthropic/claude-haiku-5-5";
const TITLE_PROMPT =
	"Name the coding task in the user's message with a 2 to 6 word title in sentence case, in the language of the message. Reply with the title only: no quotes, no trailing period.";

type Effort = "low" | "medium" | "high" | "xhigh" | "max";

interface RoleConfig {
	effort: Effort;
	criteria: string;
	contenders: string[];
	fanout?: boolean;
}

interface Config {
	classifier: { provider: string; model: string; minConfidence: number };
	titler?: string;
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

function isWhodunit(model: { provider: string; id: string } | undefined): boolean {
	return model?.provider === PROVIDER && model.id === MODEL_ID;
}

/** What Jev reads of a prompt: its text, plus a note for attached images, which it can't see. */
function judgedText(text: string, images: number): string {
	if (images === 0) return text;
	const note = `[${images === 1 ? "An image is" : `${images} images are`} attached to this request.]`;
	return text.trim() ? `${text}\n\n${note}` : note;
}

function lastUserText(messages: readonly Message[]): string {
	const content = messages.filter((message) => message.role === "user").at(-1)?.content ?? "";
	if (typeof content === "string") return content;
	const text = content.flatMap((block) => (block.type === "text" ? [block.text] : [])).join("\n");
	return judgedText(text, content.filter((block) => block.type === "image").length);
}

const IMAGE_EXTENSIONS: Record<string, string> = { "image/png": "png", "image/jpeg": "jpg", "image/gif": "gif", "image/webp": "webp" };

/** Writes the images where a fan-out's children can attach them as `@path`; `cleanup` removes them. */
function stageImages(images: readonly ImageContent[]): { paths: string[]; cleanup: () => void } {
	if (images.length === 0) return { paths: [], cleanup: () => undefined };
	const dir = mkdtempSync(join(tmpdir(), "whodunit-images-"));
	const paths = images.map((image, index) => {
		const path = join(dir, `image-${index + 1}.${IMAGE_EXTENSIONS[image.mimeType] ?? "png"}`);
		writeFileSync(path, Buffer.from(image.data, "base64"));
		return path;
	});
	return { paths, cleanup: () => rmSync(dir, { recursive: true, force: true }) };
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

async function generateTitle(
	ctx: ExtensionContext,
	agent: string,
	prompt: string,
	images: readonly ImageContent[],
	signal: AbortSignal,
): Promise<string | undefined> {
	const { provider, id } = splitAgent(agent);
	const model = ctx.modelRegistry.find(provider, id);
	if (!model) return undefined;
	const content: (TextContent | ImageContent)[] = [{ type: "text", text: prompt.slice(0, 4_000) || "(no text, only the attached images)" }, ...images];
	const reply = await ctx.modelRegistry
		.streamSimple(model, { systemPrompt: TITLE_PROMPT, messages: [{ role: "user", content, timestamp: Date.now() }] }, { maxTokens: 40, signal })
		.result();
	if (reply.stopReason !== "stop") return undefined;
	const title = textOf(reply).split("\n")[0].trim().replace(/^["'`*]+|["'`*.]+$/g, "");
	return title || undefined;
}

async function confirmRole(ctx: ExtensionContext, config: Config, classification: Classification): Promise<Classification> {
	if (classification.confidence >= config.classifier.minConfidence || !ctx.hasUI) return classification;
	const suggested = `${classification.role} (suggested, ${Math.round(classification.confidence * 100)}%)`;
	const others = Object.keys(config.roles).filter((name) => name !== classification.role);
	const choice = await ctx.ui.select("Jev isn't sure. Which role fits?", [suggested, ...others]);
	if (!choice || choice === suggested) return classification;
	return { ...classification, role: choice, source: "picker" };
}

async function askScore(ctx: ExtensionContext): Promise<Ending | undefined> {
	const picked = await ctx.ui.select(END_TITLE, SCORES.map(({ label }) => label));
	return SCORES.find(({ label }) => label === picked);
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
	images: readonly string[],
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
			...images.map((path) => `@${path}`),
			prompt || "See the attached images.",
		];
		const child = spawn("pi", args, { cwd, env: { ...process.env, WHODUNIT_CHILD: "1" }, stdio: ["ignore", "pipe", "pipe"] });
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
	images: readonly ImageContent[],
	children: Set<ChildProcess>,
): Promise<Deal> {
	const role = config.roles[classification.role];
	const order = shuffle(role.contenders);
	const started = Date.now();
	const status: string[] = order.map(() => "running");
	const render = () => {
		const elapsed = Math.round((Date.now() - started) / 1000);
		const parts = order.map((_agent, index) => `${LABELS[index]} ${status[index]}`);
		ctx.ui.setWidget(PROVIDER, [`fan-out · ${parts.join(" · ")} · ${elapsed}s`]);
	};
	render();
	const ticker = setInterval(render, 1000);
	const staged = stageImages(images);
	const results = await Promise.all(
		order.map(async (agent, index) => {
			const result = await runChild(agent, role.effort, prompt, staged.paths, ctx.cwd, children);
			status[index] = result.ok ? "done" : "failed";
			render();
			return { ...result, label: LABELS[index] };
		}),
	).finally(staged.cleanup);
	clearInterval(ticker);
	ctx.ui.setWidget(PROVIDER, undefined);

	pi.appendEntry(FANOUT_VIEW, {
		role: classification.role,
		prompt,
		images: images.length,
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
	// The images sit with the request, so the winner still sees them on follow-ups. Joined, the
	// text blocks read "User request:\n\n<prompt>\n\nYour answer:", which the app parses.
	const content: (TextContent | ImageContent)[] = [
		{ type: "text", text: `User request:\n\n${prompt}` },
		...images,
		{ type: "text", text: `\n\nYour answer:\n\n${winner.text}` },
	];
	pi.sendMessage(
		{
			customType: FANOUT_PICK,
			content,
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
	if (process.env.WHODUNIT_CHILD) return;

	const children = new Set<ChildProcess>();
	const titling = new AbortController();
	let forcedRole: string | undefined;

	const footerLabel = (ctx: ExtensionContext) => {
		const { agents } = loadConfig();
		return () => {
			const deal = findEntry<Deal>(ctx, DEAL);
			const done = findEntry<{ score: number }>(ctx, DONE);
			if (!deal) return forcedRole ? `whodunit · ${forcedRole} (forced) · waiting for first prompt` : "whodunit · waiting for first prompt";
			if (!done) return "whodunit · /done ends the run";
			return `whodunit · ${deal.role} · ${deal.size} · ${done.score}/5 · it was ${agents[deal.agent] ?? deal.agent} · ended`;
		};
	};

	const recordDeal = (ctx: ExtensionContext, deal: Deal) => {
		pi.appendEntry(DEAL, deal);
		appendRun({ type: "deal", ...sessionRef(ctx), role: deal.role, agent: deal.agent, size: deal.size, source: deal.source, confidence: deal.confidence });
	};

	const cardDeal = async (config: Config, classification: Classification): Promise<Deal> => {
		const role = config.roles[classification.role];
		return {
			role: classification.role,
			agent: await dealCard(DECKS_PATH, classification.role, role.contenders, () => shuffle(role.contenders)),
			effort: role.effort,
			size: classification.size,
			confidence: classification.confidence,
			probabilities: classification.probabilities,
			source: classification.source,
			at: new Date().toISOString(),
		};
	};

	const nameSession = (ctx: ExtensionContext, config: Config, prompt: string, images: readonly ImageContent[]) => {
		if (pi.getSessionName()) return;
		generateTitle(ctx, config.titler ?? TITLER, prompt, images, titling.signal)
			.then((title) => {
				if (title && !pi.getSessionName()) pi.setSessionName(title);
			})
			.catch(() => undefined);
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
		name: "Whodunit",
		thinkingLevels: ["medium"],
		contextWindow: 272_000,
		maxTokens: 128_000,
		async route(request, ctx) {
			const config = loadConfig();
			let deal = findEntry<Deal>(ctx, DEAL);
			if (!deal && request.reason !== "direct") {
				const classification = await pickClassification(ctx, config, lastUserText(request.messages));
				deal = await cardDeal(config, classification);
				recordDeal(ctx, deal);
			}
			const agent = deal?.agent ?? "anthropic/claude-opus-5-5";
			const effort = deal?.effort ?? "medium";
			const { provider, id } = splitAgent(agent);
			const model = ctx.modelRegistry.find(provider, id);
			if (!model) throw new Error(`whodunit: ${agent} is not in the catalog`);
			return { model, thinkingLevel: effort };
		},
	});

	pi.on("input", async (event, ctx) => {
		if (event.source === "extension") return { action: "continue" };
		if (!isWhodunit(ctx.model)) return { action: "continue" };
		if (findEntry(ctx, DONE)) {
			if (ctx.hasUI) ctx.ui.notify("This run has ended. /new for the next task.", "info");
			return { action: "handled" };
		}
		if (findEntry<Deal>(ctx, DEAL)) return { action: "continue" };

		const config = loadConfig();
		const images = event.images ?? [];
		nameSession(ctx, config, event.text, images);
		const classification = await pickClassification(ctx, config, judgedText(event.text, images.length));
		const role = config.roles[classification.role];
		const fanoutReady = role?.fanout && ctx.hasUI && role.contenders.length <= LABELS.length;
		if (!fanoutReady) {
			recordDeal(ctx, await cardDeal(config, classification));
			setFooter(ctx, footerLabel(ctx));
			return { action: "continue" };
		}
		const deal = await fanOut(pi, ctx, config, classification, event.text, images, children);
		recordDeal(ctx, deal);
		setFooter(ctx, footerLabel(ctx));
		return { action: "handled" };
	});

	pi.on("session_start", async (_event, ctx) => {
		forcedRole = undefined;
		if (!isWhodunit(ctx.model)) return;
		setFooter(ctx, footerLabel(ctx));
		if (!process.env.EXA_API_KEY && ctx.hasUI) ctx.ui.notify("EXA_API_KEY isn't set, so scout fan-outs run without web search.", "warning");
	});

	pi.on("model_select", async (event, ctx) => {
		if (isWhodunit(event.model)) {
			setFooter(ctx, footerLabel(ctx));
			return;
		}
		if (findEntry<Deal>(ctx, DEAL) && !findEntry(ctx, DONE) && !findEntry(ctx, SWITCH)) {
			pi.appendEntry(SWITCH, { to: `${event.model?.provider}/${event.model?.id}`, at: new Date().toISOString() });
		}
		if (ctx.hasUI) ctx.ui.setFooter(undefined);
	});

	pi.on("session_shutdown", async () => {
		titling.abort();
		for (const child of children) child.kill("SIGTERM");
	});

	pi.registerEntryRenderer(FANOUT_VIEW, (entry, _options, theme) => {
		const data = entry.data as { role: string; prompt: string; images?: number; answers: { label: string; ok: boolean; error?: string; text: string; searched?: boolean }[] };
		const box = new Box(1, 1, (text) => theme.bg("customMessageBg", text));
		box.addChild(new Text(theme.fg("dim", judgedText(data.prompt, data.images ?? 0))));
		box.addChild(new Text(""));
		box.addChild(new Text(theme.bold("Fan-out: pick the best answer")));
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
		description: "Force a role before the first prompt: /role <name>. The dealt role stays hidden until /done",
		getArgumentCompletions: (prefix: string) => {
			const names = Object.keys(loadConfig().roles).filter((name) => name.startsWith(prefix));
			return names.length > 0 ? names.map((name) => ({ value: name, label: name })) : null;
		},
		handler: async (args, ctx) => {
			const config = loadConfig();
			const deal = findEntry<Deal>(ctx, DEAL);
			const requested = args.trim();
			if (!requested) {
				if (!deal) {
					ctx.ui.notify(forcedRole ? `Next prompt goes to ${forcedRole}` : "No role yet. Jev picks one from the first prompt.", "info");
					return;
				}
				const ended = findEntry(ctx, DONE);
				ctx.ui.notify(ended ? `Role: ${deal.role}` : "The role stays hidden until /done.", "info");
				return;
			}
			if (deal) {
				ctx.ui.notify("This session already has a role. Use /new for a new task.", "warning");
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
		description: "End this run: score it, seal the session, then see the role and who did it",
		handler: async (_args, ctx) => {
			const config = loadConfig();
			const deal = findEntry<Deal>(ctx, DEAL);
			if (!deal) {
				ctx.ui.notify("No whodunit run in this session yet.", "warning");
				return;
			}
			const name = config.agents[deal.agent] ?? deal.agent;
			const previous = findEntry<{ outcome: string; score: number }>(ctx, DONE);
			if (previous) {
				ctx.ui.notify(`Already ended: ${previous.score}/5, ${previous.outcome}. It was ${name} (${deal.role}).`, "info");
				return;
			}
			const switched = findEntry<{ to: string }>(ctx, SWITCH);
			const chosen = switched ? SWITCHED : await askScore(ctx);
			if (!chosen) return;
			if (switched) ctx.ui.notify(`You switched to ${switched.to} mid-run, so this run counts as 1/5, failed.`, "warning");
			const record = { outcome: chosen.outcome, score: chosen.score, switched: Boolean(switched) };
			pi.appendEntry(DONE, { ...record, at: new Date().toISOString() });
			appendRun({ type: "done", ...sessionRef(ctx), role: deal.role, agent: deal.agent, ...record });
			setFooter(ctx, footerLabel(ctx));
			ctx.ui.notify(`It was ${name}, dealt as ${deal.role}. Logged ${chosen.score}/5, ${chosen.outcome}.`, "info");
		},
	});
}
