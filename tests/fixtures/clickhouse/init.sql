CREATE DATABASE IF NOT EXISTS lazydb_test;

CREATE USER IF NOT EXISTS lazydb_test IDENTIFIED WITH plaintext_password BY 'lazydb_test';
GRANT CURRENT GRANTS ON *.* TO lazydb_test WITH GRANT OPTION;

CREATE TABLE IF NOT EXISTS lazydb_test.fixture_events
(
    id UInt64,
    label String,
    created_at DateTime DEFAULT now()
)
ENGINE = MergeTree
ORDER BY id;

INSERT INTO lazydb_test.fixture_events (id, label)
SELECT number, concat('fixture-', toString(number))
FROM numbers(3);

CREATE USER IF NOT EXISTS lazydb_restricted
IDENTIFIED WITH no_password;
GRANT SELECT ON lazydb_test.fixture_events TO lazydb_restricted;
