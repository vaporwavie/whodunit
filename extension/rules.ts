// Pure rules the Bend spec in laws/ mirrors; laws/check.mjs compares the two.

export interface Ending {
	score: number;
	outcome: "nailed it" | "needed my fixes" | "failed";
	label: string;
}

export const SCORES: Ending[] = [
	{ score: 5, outcome: "nailed it", label: "5 · nailed it, great" },
	{ score: 4, outcome: "nailed it", label: "4 · nailed it, rough edges" },
	{ score: 3, outcome: "needed my fixes", label: "3 · needed small fixes" },
	{ score: 2, outcome: "needed my fixes", label: "2 · needed big fixes" },
	{ score: 1, outcome: "failed", label: "1 · failed" },
];

export const SWITCHED: Ending = SCORES[SCORES.length - 1];

export function draw<T>(deck: readonly T[], contenders: readonly T[], refill: () => T[]): { card: T; rest: T[] } {
	const remaining = deck.filter((agent) => contenders.includes(agent));
	const [card, ...rest] = remaining.length > 0 ? remaining : refill();
	return { card, rest };
}
