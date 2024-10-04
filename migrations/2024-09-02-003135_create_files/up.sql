CREATE TABLE files (
    id          SERIAL PRIMARY KEY,
    file_name   VARCHAR NOT NULL,
    content     TEXT NOT NULL
)

CREATE TABLE in_progress_files (
	id			SERIAL PRIMARY KEY
	browser_id  SERIAL PRIMARY KEY
)
