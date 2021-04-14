CREATE TABLE IF NOT EXISTS test_table(
    test_int INTEGER NOT NULL,
    test_nullint INTEGER,
    test_str TEXT,
    test_float DOUBLE PRECISION,
    test_bool BOOLEAN,
    test_uuid UUID NOT NULL
);

INSERT INTO test_table VALUES (1, 3, 'str1', NULL, TRUE, '86b494cc-96b2-11eb-9298-3e22fbb9fe9d');
INSERT INTO test_table VALUES (2, NULL, 'str2', 2.2, FALSE, '86b49b84-96b2-11eb-9298-3e22fbb9fe9d');
INSERT INTO test_table VALUES (0, 5, 'a', 3.1, NULL, '86b49c42-96b2-11eb-9298-3e22fbb9fe9d');
INSERT INTO test_table VALUES (3, 7, 'b', 3, FALSE, '86b49cce-96b2-11eb-9298-3e22fbb9fe9d');
INSERT INTO test_table VALUES (4, 9, 'c', 7.8, NULL, '59e06bb4-9d02-11eb-9021-3e22fbb9fe9d');
INSERT INTO test_table VALUES (1314, 2, NULL, -10, TRUE, '5fd2de58-9d02-11eb-9021-3e22fbb9fe9d');

CREATE TABLE IF NOT EXISTS test_str(
    id INTEGER NOT NULL,
    test_language TEXT,
    test_hello TEXT
);

INSERT INTO test_str VALUES (0, 'English', 'Hello');
INSERT INTO test_str VALUES (1, '中文', '你好');
INSERT INTO test_str VALUES (2, '日本語', 'こんにちは');
INSERT INTO test_str VALUES (3, 'русский', 'Здра́вствуйте');
INSERT INTO test_str VALUES (4, 'Emoji', '😁😂😜');
INSERT INTO test_str VALUES (5, 'Latin1', '¥§¤®ð');
INSERT INTO test_str VALUES (6, 'Extra', 'y̆');
INSERT INTO test_str VALUES (7, 'Mixed', 'Ha好ち😁ðy̆');
