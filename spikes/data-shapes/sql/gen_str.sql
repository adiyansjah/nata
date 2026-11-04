-- (2) STRING-HEAVY. @M@ fact rows in random order (so a row group holds an arbitrary mix of values: dictionary-unfriendly).
--   s_hi     millions of distinct values: about 1 - e^-1.5 = 78 percent of @MH@ possible 20-byte values hit
--   s_long   100..500 bytes, about @ML@ distinct values (length = 100 + y % 401)
--   s_empty  40 percent '' (empty, not NULL), 3 percent NULL, the rest 'e' + number
--   s_utf8   non-ASCII, 1 to 4 bytes per character, composed from 14 building blocks; includes strings that are
--            canonically equal but differ in bytes (precomposed e-acute against 'e' + combining acute)
--   s_last   100 bytes, strings differ ONLY in the last byte (62 values) within each of 2000 prefixes
--   s_first  the mirror image: differ only in the first byte
--   s_len    'a' repeated 1..300 times: differ only in length
--   l_shipinstruct  about @MI@ distinct 33-byte values, read as a DICTIONARY column by the hash-ops reader (name
--            maps to Dictionary<Int32, Utf8>): a dictionary as large as the data, which Parquet cannot keep as one
--            dictionary page, so the reader meets dictionary pages and plain fallback pages
COPY (
  SELECT i AS id,
         str_hi(ri(i, 1, @MH@)) AS s_hi,
         str_long(ri(i, 2, @ML@)) AS s_long,
         str_e(u01(i, 3), hash(i, 4)) AS s_empty,
         u8part(hash(i, 5)) || u8part(hash(i, 6)) || (hash(i, 7) % 3000::UBIGINT)::VARCHAR AS s_utf8,
         str_last(ri(i, 8, 2000), ri(i, 9, 62)) AS s_last,
         str_first(ri(i, 8, 2000), ri(i, 9, 62)) AS s_first,
         repeat('a', (1 + ri(i, 10, 300))::INTEGER) AS s_len,
         str_instr(ri(i, 11, @MI@)) AS l_shipinstruct,
         ri(i, 12, 1000) AS v,
         ri(i, 13, 1000001) - 500000 AS w
  FROM range(@M@) t(i)
) TO '@DIR@/str_fact.parquet' (FORMAT PARQUET, ROW_GROUP_SIZE 122880);

-- join build sides
COPY (SELECT str_long(y) AS dkey, y % 1000 AS dpay, 'T' || (y % 50)::VARCHAR AS dtag FROM range(@ML@) t(y))
  TO '@DIR@/str_dim.parquet' (FORMAT PARQUET, ROW_GROUP_SIZE 122880);
COPY (SELECT str_hi(x) AS dkey2, x % 1000 AS dpay2 FROM range(@MH@) t(x))
  TO '@DIR@/str_dim2.parquet' (FORMAT PARQUET, ROW_GROUP_SIZE 122880);
-- only 90 percent of the dictionary values exist here: a tenth of the probe rows find nothing
COPY (SELECT str_instr(x) AS dkey3, x % 777 AS dpay3 FROM range(@MI@ * 9 // 10) t(x))
  TO '@DIR@/str_dim3.parquet' (FORMAT PARQUET, ROW_GROUP_SIZE 122880);
-- every distinct s_utf8 of the fact once (read back from the file just written)
COPY (SELECT s_utf8 AS dku, (hash(s_utf8) % 100::UBIGINT)::BIGINT AS dpu
      FROM (SELECT DISTINCT s_utf8 FROM read_parquet('@DIR@/str_fact.parquet')))
  TO '@DIR@/str_dim4.parquet' (FORMAT PARQUET, ROW_GROUP_SIZE 122880);
-- 100 rows with the same empty/NULL/short mix as s_empty: about 40 build rows are '' (a chain of 40), 3 are NULL
COPY (SELECT str_e(u01(i, 31), hash(i, 32)) AS sk, i % 13 AS sg FROM range(100) t(i))
  TO '@DIR@/str_small.parquet' (FORMAT PARQUET, ROW_GROUP_SIZE 122880);
