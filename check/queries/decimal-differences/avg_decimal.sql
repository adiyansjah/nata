-- tolerance: 1e-7
SELECT avg(a) AS v FROM (VALUES (1.00::DECIMAL(15,2)), (2.00::DECIMAL(15,2)), (2.00::DECIMAL(15,2))) t(a)
