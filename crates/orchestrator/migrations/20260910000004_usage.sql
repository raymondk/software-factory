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
