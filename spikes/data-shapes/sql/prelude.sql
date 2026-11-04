-- Deterministic helpers shared by every generator. Nothing here uses random(): every value is a function of the
-- row number i and a salt, through DuckDB's hash(), so two runs (and two machines of one architecture) write the
-- same data. Placeholders @NAME@ are filled in by gcp-check.sh (see its header).
CREATE OR REPLACE MACRO hu(i, salt) AS hash(i, salt);                                        -- UBIGINT
CREATE OR REPLACE MACRO u01(i, salt) AS (hash(i, salt) % 4294967296::UBIGINT)::DOUBLE / 4294967296.0;   -- [0, 1)
CREATE OR REPLACE MACRO ri(i, salt, n) AS (hash(i, salt) % (n)::UBIGINT)::BIGINT;           -- [0, n)
-- Zipf-like rank in [1, n] by inverting the continuous power-law CDF (exponent s = 1.0 and 1.5): P(rank = r) is
-- about r^-s. Rank 1 gets about 5 percent (s = 1.0, n = 1e6) and about 29 percent (s = 1.5) of the rows.
CREATE OR REPLACE MACRO zipf10(u, n) AS LEAST((n)::BIGINT, GREATEST(1::BIGINT, floor(exp(u * ln((n)::DOUBLE)))::BIGINT));
CREATE OR REPLACE MACRO zipf15(u, n) AS LEAST((n)::BIGINT, GREATEST(1::BIGINT, floor(pow((pow((n)::DOUBLE, -0.5) - 1.0) * u + 1.0, -2.0))::BIGINT));
-- NULL with probability p (p = 0 never, p = 1 always, because u01 < 1).
CREATE OR REPLACE MACRO nul(i, salt, p) AS u01(i, salt) < p;
-- strings
CREATE OR REPLACE MACRO alnum62(c) AS substr('0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz', (c)::BIGINT + 1, 1);
CREATE OR REPLACE MACRO str_hi(x) AS 'u' || lpad((x)::VARCHAR, 12, '0') || '#ex.org';
CREATE OR REPLACE MACRO str_long(y) AS substr(repeat(md5((y)::VARCHAR), 16), 1, (100 + (y) % 401)::INTEGER);
CREATE OR REPLACE MACRO str_last(p, c) AS 'P' || lpad((p)::VARCHAR, 5, '0') || repeat('x', 90) || alnum62(c);
CREATE OR REPLACE MACRO str_first(p, c) AS alnum62(c) || 'P' || lpad((p)::VARCHAR, 5, '0') || repeat('x', 90);
CREATE OR REPLACE MACRO str_instr(x) AS 'INSTR ' || lpad((x)::VARCHAR, 7, '0') || repeat('-', 20);
-- 40 percent empty strings, 3 percent NULL, the rest short; u is a uniform in [0, 1), h a hash
CREATE OR REPLACE MACRO str_e(u, h) AS CASE WHEN u < 0.40 THEN '' WHEN u < 0.43 THEN NULL ELSE 'e' || (h % 1000::UBIGINT)::VARCHAR END;
-- Non-ASCII building blocks, written with chr() so the bytes do not depend on the encoding of this file:
-- e-acute, u-diaeresis, sharp s, Japanese, Chinese, Korean, Russian, 1 emoji (4 bytes), a ZWJ emoji sequence,
-- 'e' + combining acute (the same text as e-acute after normalisation, different bytes), I with dot, DZ with caron
-- (titlecase), a 4-byte mathematical letter, the fi ligature.
CREATE OR REPLACE MACRO u8part(k) AS list_extract([
  chr(233), chr(252), chr(223),
  chr(26085) || chr(26412) || chr(35486), chr(20013) || chr(25991), chr(54620) || chr(44397) || chr(50612),
  chr(1055) || chr(1088) || chr(1080) || chr(1074) || chr(1077) || chr(1090),
  chr(128578), chr(128105) || chr(8205) || chr(128187), chr(101) || chr(769), chr(304), chr(453), chr(120088), chr(64257)
], 1 + ((k) % 14::UBIGINT)::INTEGER);
