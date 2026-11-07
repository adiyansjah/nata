SELECT
  d.datname as "Name",
  pg_catalog.pg_get_userbyid(d.datdba) as "Owner",
  pg_catalog.pg_encoding_to_char(d.encoding) as "Encoding",
  CASE d.datlocprovider WHEN 'b' THEN 'builtin' WHEN 'c' THEN 'libc' WHEN 'i' THEN 'icu' END AS "Locale Provider",
  d.datcollate as "Collate",
  d.datctype as "Ctype",
  d.daticulocale as "Locale",
  d.daticurules as "ICU Rules",
  CASE WHEN pg_catalog.array_length(d.datacl, 1) = 0 THEN '(none)' ELSE pg_catalog.array_to_string(d.datacl, E'\n') END AS "Access privileges"
FROM pg_catalog.pg_database d
ORDER BY 1;
