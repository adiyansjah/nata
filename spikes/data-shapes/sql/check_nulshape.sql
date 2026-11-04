-- The shape of the NULLs: whole NULL batches, a column NULL in every row group but one, all-NULL columns.
WITH f AS (SELECT * FROM read_parquet('@DIR@/nul_shape.parquet')),
 md AS (SELECT * FROM parquet_metadata('@DIR@/nul_shape.parquet'))
SELECT 'rows', count(*), @NS@, @NS@ FROM f
UNION ALL SELECT 'row_groups', count(DISTINCT row_group_id), 3, 1000000 FROM md
UNION ALL SELECT 'row_group_rows_of_the_first_group', min(row_group_num_rows) FILTER (WHERE row_group_id = 0), @RGS@, @RGS@ FROM md
UNION ALL SELECT 'row_groups_where_kr_has_a_value (exactly 1)', count(*), 1, 1 FROM md WHERE path_in_schema = 'kr' AND stats_null_count < num_values
UNION ALL SELECT 'row_groups_where_vr_has_a_value (exactly 1)', count(*), 1, 1 FROM md WHERE path_in_schema = 'vr' AND stats_null_count < num_values
UNION ALL SELECT 'row_groups_where_kall_has_a_value (none)', count(*), 0, 0 FROM md WHERE path_in_schema = 'kall' AND stats_null_count < num_values
UNION ALL SELECT 'kr_non_null_rows', count(kr), @RGS@ // 2 - 1, @RGS@ // 2 + 1 FROM f
UNION ALL SELECT 'all_NULL_aligned_8192_blocks_of_kb_al', count(*), 1, 1000000 FROM (SELECT id // 8192 AS b FROM f GROUP BY b HAVING count(kb_al) = 0)
UNION ALL SELECT 'all_valid_aligned_8192_blocks_of_kb_al', count(*), 1, 1000000 FROM (SELECT id // 8192 AS b FROM f GROUP BY b HAVING count(kb_al) = count(*))
UNION ALL SELECT 'all_NULL_5000_blocks_of_kb_un', count(*), 1, 1000000 FROM (SELECT id // 5000 AS b FROM f GROUP BY b HAVING count(kb_un) = 0)
UNION ALL SELECT 'kall_null_share', avg((kall IS NULL)::INT), 1, 1 FROM f
UNION ALL SELECT 'vall_null_share', avg((vall IS NULL)::INT), 1, 1 FROM f
UNION ALL SELECT 'v_null_share', avg((v IS NULL)::INT), 0, 0 FROM f;
