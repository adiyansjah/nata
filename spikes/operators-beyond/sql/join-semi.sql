select count(*) as n from customer where exists (select 1 from orders where o_custkey = c_custkey)
