DROP TRIGGER IF EXISTS notes_fts_ai;
DROP TRIGGER IF EXISTS notes_fts_ad;
DROP TRIGGER IF EXISTS notes_fts_au;
DROP TABLE IF EXISTS notes_fts;

CREATE VIRTUAL TABLE notes_fts USING fts5(
    note_id UNINDEXED,
    note_title,
    body,
    tokenize = 'unicode61'
);

CREATE TRIGGER notes_fts_ai
AFTER INSERT ON notes
BEGIN
    INSERT INTO notes_fts(rowid, note_id, note_title, body)
    SELECT new.rowid, new.id, new.note_title, new.body
    WHERE new.access_mode = 'none';
END;

CREATE TRIGGER notes_fts_ad
AFTER DELETE ON notes
BEGIN
    DELETE FROM notes_fts WHERE rowid = old.rowid;
END;

CREATE TRIGGER notes_fts_au
AFTER UPDATE ON notes
BEGIN
    DELETE FROM notes_fts WHERE rowid = old.rowid;
    INSERT INTO notes_fts(rowid, note_id, note_title, body)
    SELECT new.rowid, new.id, new.note_title, new.body
    WHERE new.access_mode = 'none';
END;

INSERT INTO notes_fts(rowid, note_id, note_title, body)
SELECT rowid, id, note_title, body
FROM notes
WHERE access_mode = 'none';
