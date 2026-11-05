-- scan: {data}/lineitem.parquet --columns l_orderkey,l_linenumber,l_shipdate,l_extendedprice --filter 'l_shipdate >= 1994-01-01' --filter 'l_shipdate < 1995-01-01'
SELECT l_orderkey, l_linenumber, l_shipdate, l_extendedprice
FROM '{data}/lineitem.parquet'
WHERE l_shipdate >= DATE '1994-01-01' AND l_shipdate < DATE '1995-01-01'
