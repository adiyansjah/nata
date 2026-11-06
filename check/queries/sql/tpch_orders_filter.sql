SELECT o_orderkey, o_totalprice, o_orderdate FROM '{data}/orders.parquet' WHERE o_totalprice > 400000 AND o_orderstatus = 'F'
