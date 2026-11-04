select count(*) as n, count(c_custkey) as c, count(o_orderkey) as o from customer right join orders on c_custkey = o_custkey
