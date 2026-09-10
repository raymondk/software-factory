CREATE TABLE tickets (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    title       TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    state       TEXT NOT NULL DEFAULT 'todo',
    rank        REAL NOT NULL,
    assignee    TEXT,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);
