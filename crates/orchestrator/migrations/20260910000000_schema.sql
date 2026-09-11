CREATE TABLE tickets (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    title       TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    state       TEXT NOT NULL DEFAULT 'todo',
    rank        REAL NOT NULL,
    assignee    TEXT,
    links       TEXT NOT NULL DEFAULT '[]',
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

CREATE TABLE comments (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    ticket_id  INTEGER NOT NULL REFERENCES tickets(id),
    author     TEXT NOT NULL,
    body       TEXT NOT NULL,
    created_at TEXT NOT NULL,
    resolved   INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX comments_ticket ON comments(ticket_id, created_at, id);

CREATE TABLE workers (
    id             TEXT PRIMARY KEY,
    worker_type    TEXT NOT NULL,
    token          TEXT NOT NULL UNIQUE,
    status         TEXT NOT NULL DEFAULT 'starting',
    created_at     TEXT NOT NULL,
    last_heartbeat TEXT
);

CREATE TABLE usage (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    ticket_id   INTEGER NOT NULL REFERENCES tickets(id),
    worker_id   TEXT NOT NULL REFERENCES workers(id),
    worker_type TEXT NOT NULL,
    tokens_in   INTEGER NOT NULL,
    tokens_out  INTEGER NOT NULL,
    cost        REAL NOT NULL,
    created_at  TEXT NOT NULL
);
CREATE INDEX usage_ticket ON usage(ticket_id);

CREATE TABLE ticket_relations (
    from_id INTEGER NOT NULL REFERENCES tickets(id),
    type    TEXT NOT NULL,
    to_id   INTEGER NOT NULL REFERENCES tickets(id),
    UNIQUE (from_id, type, to_id)
);
CREATE INDEX ticket_relations_to ON ticket_relations(to_id);
