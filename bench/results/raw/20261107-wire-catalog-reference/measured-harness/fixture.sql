CREATE SCHEMA empty;
CREATE SCHEMA other;
CREATE SCHEMA "Quoted Schema";
CREATE TABLE public.all_types(tiny text, b boolean, s smallint, i integer, l bigint, f real, d double precision, n numeric(18,3), day date, txt text);
INSERT INTO public.all_types VALUES ('-128',true,-32768,-2147483648,-9223372036854775808,1.25,1.25,123.450,'2000-01-01',E'quote" comma, tab\t newline\n'),(NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL),('',false,0,0,0,0,0,0,'0001-01-01','');
CREATE TABLE other.all_types (v bigint);
CREATE TABLE "Quoted Schema"."Mixed Table" ("Mixed Column" numeric(10,2));
CREATE TABLE million AS SELECT i::bigint AS v FROM generate_series(0,999999) i;
ANALYZE;
