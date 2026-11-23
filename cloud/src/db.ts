// Small D1 helpers. Every statement that touches user data is written in routes.ts / auth.ts
// with `user_id = ?` bound from the verified token, never from the request.

export function stmt(db: D1Database, sql: string, ...binds: unknown[]): D1PreparedStatement {
  return db.prepare(sql).bind(...binds);
}

export async function one<T>(db: D1Database, sql: string, ...binds: unknown[]): Promise<T | null> {
  return stmt(db, sql, ...binds).first<T>();
}

export async function all<T>(db: D1Database, sql: string, ...binds: unknown[]): Promise<T[]> {
  const r = await stmt(db, sql, ...binds).all<T>();
  return r.results;
}

/** Runs a statement and returns how many rows it changed. */
export async function exec(db: D1Database, sql: string, ...binds: unknown[]): Promise<number> {
  const r = await stmt(db, sql, ...binds).run();
  return r.meta.changes ?? 0;
}

/** Newest-wins clamp: a clock that is far ahead must not pin a value forever. */
export const MAX_FUTURE_SKEW_MS = 10 * 60 * 1000;
export function clampFuture(ts: number, now: number): number {
  return Math.min(ts, now + MAX_FUTURE_SKEW_MS);
}
