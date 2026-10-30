-- Read markers for agent inbox items (e.g. 'run:<uuid>', 'merge:<uuid>').
-- Agent runs also use coding_agent_turns.seen; this covers the rest.
CREATE TABLE agent_inbox_reads (
    id      TEXT PRIMARY KEY NOT NULL,
    read_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
);
