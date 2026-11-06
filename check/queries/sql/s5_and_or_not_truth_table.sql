SELECT id, f AND (i > 2) AS a, f OR (i > 2) AS b, NOT f AS c, NOT (f AND (i > 2)) AS d, (f OR NULL) AS e, (f AND NULL) AS g FROM '{data}/sqlcheck/types.parquet'
