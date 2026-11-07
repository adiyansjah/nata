import java.sql.*;
public class Clients {
 interface Probe {void run() throws Exception;}
 static void probe(String name, Probe p) {try {p.run();System.out.println(name+": PASS");} catch(Exception e) {System.out.println(name+": FAIL "+(e instanceof SQLException ? ((SQLException)e).getSQLState() : e.getClass().getSimpleName()));}}
 static void rows(ResultSet r) throws Exception {try(r) {while(r.next()) for(int i=1;i<=r.getMetaData().getColumnCount();i++) r.getObject(i);}}
 public static void main(String[] a) throws Exception {
  try(Connection c=DriverManager.getConnection("jdbc:postgresql://127.0.0.1:"+a[0]+"/nata?user=nata")) {
   System.out.println("connect: PASS driver="+c.getMetaData().getDriverVersion());
   String q="SELECT v FROM nata.public.all_types LIMIT 3";
   probe("select",()->{try(PreparedStatement s=c.prepareStatement(q)){for(int run=0;run<10;run++)try(ResultSet r=s.executeQuery()){int i=0;while(r.next()) if(r.getLong(1)!=i++)throw new Exception("value");if(i!=3)throw new Exception("count");}}});
   DatabaseMetaData m=c.getMetaData();
   probe("getCatalogs",()->rows(m.getCatalogs())); probe("getSchemas",()->rows(m.getSchemas()));
   probe("getTables",()->rows(m.getTables(null,null,"%",null)));probe("getColumns",()->rows(m.getColumns(null,null,"%","%")));
   probe("getPrimaryKeys",()->rows(m.getPrimaryKeys(null,"public","all_types")));probe("getIndexInfo",()->rows(m.getIndexInfo(null,"public","all_types",false,false)));
   probe("getTypeInfo",()->rows(m.getTypeInfo()));probe("getFunctions",()->rows(m.getFunctions(null,null,"%")));
   probe("positive-fetch",()->{c.setAutoCommit(false);try(Statement s=c.createStatement()){s.setFetchSize(2);rows(s.executeQuery(q));}finally{c.rollback();c.setAutoCommit(true);}});
   probe("timeout",()->{try(Statement s=c.createStatement()){s.execute("SET statement_timeout=1");try {rows(s.executeQuery("SELECT v FROM nata.public.all_types"));throw new Exception("no timeout");} catch(SQLException e) {if(!"57014".equals(e.getSQLState()))throw e;} finally{s.execute("SET statement_timeout=0");}}});
   probe("cancel",()->{try(Statement s=c.createStatement()){Thread t=new Thread(()->{try{Thread.sleep(30);s.cancel();}catch(Exception e){throw new RuntimeException(e);}});t.start();try{rows(s.executeQuery("SELECT v FROM nata.public.all_types"));throw new Exception("no cancel");}catch(SQLException e){if(!"57014".equals(e.getSQLState()))throw e;}finally{t.join();}rows(s.executeQuery(q));}});
   probe("copy-text",()->{java.io.ByteArrayOutputStream out=new java.io.ByteArrayOutputStream();c.unwrap(org.postgresql.PGConnection.class).getCopyAPI().copyOut("COPY ("+q+") TO STDOUT",out);if(!out.toString(java.nio.charset.StandardCharsets.UTF_8).equals("0\n1\n2\n"))throw new Exception("text copy");});
   probe("copy-csv",()->{java.io.ByteArrayOutputStream out=new java.io.ByteArrayOutputStream();c.unwrap(org.postgresql.PGConnection.class).getCopyAPI().copyOut("COPY ("+q+") TO STDOUT (FORMAT csv, HEADER true)",out);if(!out.toString(java.nio.charset.StandardCharsets.UTF_8).equals("v\n0\n1\n2\n"))throw new Exception("csv copy");});
   probe("copy-table",()->{java.io.ByteArrayOutputStream out=new java.io.ByteArrayOutputStream();c.unwrap(org.postgresql.PGConnection.class).getCopyAPI().copyOut("COPY nata.public.all_types TO STDOUT WITH (FORMAT csv, HEADER true)",out);try(java.io.BufferedReader r=new java.io.BufferedReader(new java.io.StringReader(out.toString(java.nio.charset.StandardCharsets.UTF_8)))){if(!"v".equals(r.readLine()))throw new Exception("header");for(int n=0;n<1000000;n++)if(!Integer.toString(n).equals(r.readLine()))throw new Exception("value");if(r.readLine()!=null)throw new Exception("count");}});
   probe("copy",()->{java.io.ByteArrayOutputStream out=new java.io.ByteArrayOutputStream();c.unwrap(org.postgresql.PGConnection.class).getCopyAPI().copyOut("COPY ("+q+") TO STDOUT (FORMAT binary)",out);byte[] b=out.toByteArray();java.nio.ByteBuffer expected=java.nio.ByteBuffer.allocate(63);expected.put(new byte[]{80,71,67,79,80,89,10,(byte)255,13,10,0}).putInt(0).putInt(0);for(int n=0;n<3;n++)expected.putShort((short)1).putInt(8).putLong(n);expected.putShort((short)-1);if(!java.util.Arrays.equals(b,expected.array()))throw new Exception("copy");});
   probe("recovery",()->{c.setAutoCommit(false);try(Statement s=c.createStatement()){try{s.executeQuery("SELECT v/0 FROM nata.public.all_types LIMIT 1");throw new Exception("no error");}catch(SQLException e){if(!"22012".equals(e.getSQLState()))throw e;}c.rollback();rows(s.executeQuery(q));}finally{c.rollback();c.setAutoCommit(true);}});
  }
 }
}
