CREATE TABLE comments (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    ticket_id  INTEGER NOT NULL REFERENCES tickets(id),
    author     TEXT NOT NULL,
    body       TEXT NOT NULL,
    created_at TEXT NOT NULL,
    resolved   INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX comments_ticket ON comments(ticket_id, created_at, id);
