select o_orderkey, o_custkey, o_totalprice, o_orderstatus,
  rank() over w as rk, dense_rank() over w as dr, sum(o_totalprice) over (partition by o_custkey) as tot
from orders window w as (partition by o_custkey order by o_orderstatus)
