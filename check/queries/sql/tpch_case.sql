SELECT l_orderkey, CASE WHEN l_quantity > 25 THEN 'big' ELSE 'small' END AS size, l_shipmode FROM '{data}/lineitem.parquet' WHERE l_shipmode IN ('AIR', 'MAIL') LIMIT 100
