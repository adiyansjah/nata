SELECT id FROM '{data}/sqlcheck/types.parquet' WHERE CASE WHEN i > 3 THEN f ELSE NOT f END
