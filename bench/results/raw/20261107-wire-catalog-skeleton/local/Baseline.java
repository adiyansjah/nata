import java.sql.*;
public class Baseline {
 interface Probe {void run() throws Exception;}
 static void probe(String name, Probe p) {try {p.run();System.out.println(name+": PASS");} catch(Exception e) {System.out.println(name+": FAIL "+(e instanceof SQLException ? ((SQLException)e).getSQLState() : e.getClass().getSimpleName()));}}
 static void rows(ResultSet r) throws Exception {try(r) {while(r.next()) for(int i=1;i<=r.getMetaData().getColumnCount();i++) r.getObject(i);}}
 public static void main(String[] a) throws Exception {
  try(Connection c=DriverManager.getConnection("jdbc:postgresql://127.0.0.1:"+a[0]+"/nata?user=nata")) {
   System.out.println("connect: PASS driver="+c.getMetaData().getDriverVersion());
   String q="SELECT v FROM '"+a[1].replace("'","''")+"' LIMIT 3";
   probe("select",()->{try(PreparedStatement s=c.prepareStatement(q)){for(int run=0;run<10;run++)try(ResultSet r=s.executeQuery()){int i=0;while(r.next()) if(r.getLong(1)!=i++)throw new Exception("value");if(i!=3)throw new Exception("count");}}});
   DatabaseMetaData m=c.getMetaData();
   probe("getCatalogs",()->rows(m.getCatalogs())); probe("getSchemas",()->rows(m.getSchemas()));
   probe("getTables",()->rows(m.getTables(null,null,"%",null)));probe("getColumns",()->rows(m.getColumns(null,null,"%","%")));
   probe("getPrimaryKeys",()->rows(m.getPrimaryKeys(null,"public","all_types")));probe("getIndexInfo",()->rows(m.getIndexInfo(null,"public","all_types",false,false)));
   probe("getTypeInfo",()->rows(m.getTypeInfo()));
   probe("positive-fetch",()->{c.setAutoCommit(false);try(Statement s=c.createStatement()){s.setFetchSize(2);rows(s.executeQuery(q));}finally{c.rollback();c.setAutoCommit(true);}});
   probe("timeout",()->{try(Statement s=c.createStatement()){s.execute("SET statement_timeout=20");}});
   probe("copy",()->{java.io.ByteArrayOutputStream out=new java.io.ByteArrayOutputStream();c.unwrap(org.postgresql.PGConnection.class).getCopyAPI().copyOut("COPY ("+q+") TO STDOUT (FORMAT binary)",out);byte[] b=out.toByteArray();java.nio.ByteBuffer expected=java.nio.ByteBuffer.allocate(63);expected.put(new byte[]{80,71,67,79,80,89,10,(byte)255,13,10,0}).putInt(0).putInt(0);for(int n=0;n<3;n++)expected.putShort((short)1).putInt(8).putLong(n);expected.putShort((short)-1);if(!java.util.Arrays.equals(b,expected.array()))throw new Exception("copy");});
   probe("error-recovery",()->{c.setAutoCommit(false);try(Statement s=c.createStatement()){try{s.executeQuery("SELECT v/0 FROM '"+a[1]+"' LIMIT 1");throw new Exception("no error");}catch(SQLException e){if(!"22012".equals(e.getSQLState()))throw e;}c.rollback();rows(s.executeQuery(q));}finally{c.rollback();c.setAutoCommit(true);}});
  }
 }
}
