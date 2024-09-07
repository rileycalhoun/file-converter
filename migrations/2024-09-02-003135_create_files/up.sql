CREATE TABLE files (
    id          SERIAL PRIMARY KEY,
    file_name   VARCHAR NOT NULL,
    content     TEXT NOT NULL
)