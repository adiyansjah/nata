import java.sql.*;
import java.math.BigDecimal;
import org.postgresql.util.PSQLException;
public class Clients4 {
 static String url, f, large;
 static Connection connect(String opts)throws Exception {return DriverManager.getConnection(url+opts,"test","");}
 public static void main(String[] a)throws Exception {
  url="jdbc:postgresql://127.0.0.1:"+a[0]+"/nata?connectTimeout=5";f=a[1];large=a[2];
  try(Connection c=connect("")) {
   System.out.println("pgjdbc "+c.getMetaData().getDriverVersion()+" Java "+System.getProperty("java.version"));
   try(PreparedStatement s=c.prepareStatement("SELECT * FROM '"+f+"'")) {
   for(int run=0;run<10;run++) { try(ResultSet r=s.executeQuery()) {
    r.next();Object[] expected={true,"-128",-32768,Integer.MIN_VALUE,Long.MIN_VALUE,-0.0f,Double.NaN,new BigDecimal("123.450"),"",Date.valueOf("1970-01-01")};
    for(int i=1;i<=10;i++) {Object v=r.getObject(i);System.out.println(i+" "+v.getClass().getSimpleName()+" "+v);if(!v.equals(expected[i-1]))throw new AssertionError("type/value "+i);}
    r.next();Object[] second={false,"127",32767,Integer.MAX_VALUE,Long.MAX_VALUE,Float.POSITIVE_INFINITY,Double.NEGATIVE_INFINITY,new BigDecimal("-0.001"),"secret-cell-a,b\né",Date.valueOf("2024-02-29")};
    for(int i=1;i<=10;i++)if(!r.getObject(i).equals(second[i-1]))throw new AssertionError("second row "+i);
    r.next();for(int i=1;i<=10;i++)if(r.getObject(i)!=null)throw new AssertionError("NULL");
   }
   }}
   System.out.println("DEFAULT ten repeats types/NULL PASS");
   try(PreparedStatement s=c.prepareStatement("SELECT si FROM '"+f+"' LIMIT 1")) {for(int i=0;i<10;i++){try(ResultSet r=s.executeQuery()){r.next();if(r.getInt(1)!=-32768)throw new AssertionError();}System.out.println("DEFAULT repeated no parameters "+(i+1)+" PASS");}}

  }
  try(Connection c=connect("")) {
   try(PreparedStatement s=c.prepareStatement("SELECT ? AS p FROM '"+f+"' LIMIT 1")) {for(int i=0;i<10;i++){s.setInt(1,7);try(ResultSet r=s.executeQuery()){r.next();if(r.getInt(1)!=7)throw new AssertionError();}System.out.println("DEFAULT prepared "+(i+1)+" PASS");}}

  }
  try(Connection c=connect("")) {
   try(PreparedStatement s=c.prepareStatement("SELECT ? AS p FROM '"+f+"' LIMIT 1")){for(int i=0;i<10;i++){s.setInt(1,7);try(ResultSet r=s.executeQuery()){r.next();if(r.getInt(1)!=7)throw new AssertionError();}}s.setNull(1,Types.INTEGER);try(ResultSet r=s.executeQuery()){r.next();if(r.getObject(1)!=null)throw new AssertionError();}}
   try(PreparedStatement s=c.prepareStatement("SELECT ? AS p FROM '"+f+"' LIMIT 1")){s.setBoolean(1,true);try(ResultSet r=s.executeQuery()){r.next();if(!Boolean.TRUE.equals(r.getObject(1)))throw new AssertionError();}}
   System.out.println("DEFAULT prepared/repeat10/NULL/boolean PASS");
   try(Statement s=c.createStatement()){try{s.executeQuery("SELECT nope FROM '"+f+"'");throw new AssertionError();}catch(PSQLException e){System.out.println("error "+e.getSQLState()+" position "+e.getServerErrorMessage().getPosition());if(!e.getSQLState().equals("42703")||e.getServerErrorMessage().getPosition()!=8)throw e;}}
   try(Statement s=c.createStatement()) {
    Thread t=new Thread(()->{try{Thread.sleep(150);s.cancel();}catch(Exception e){throw new RuntimeException(e);}});t.start();
    try{s.executeQuery("SELECT s FROM '"+large+"'");throw new AssertionError("not cancelled");}catch(SQLException e){System.out.println("cancel "+e.getSQLState());if(!e.getSQLState().equals("57014"))throw e;}finally{t.join();}
   }
  }
  try(Connection c=connect("")) {
   c.setAutoCommit(false);
   try(PreparedStatement s=c.prepareStatement("SELECT s FROM '"+large+"'")) {
    s.setFetchSize(100);
    try { s.executeQuery(); throw new AssertionError("fetchSize should be refused"); }
    catch(SQLException e) { if(!e.getSQLState().equals("0A000"))throw e; System.out.println("fetchSize=100 autocommit=false: "+e.getSQLState()); }
   }
   c.rollback();
   try(Statement s=c.createStatement();ResultSet r=s.executeQuery("SELECT 1")) { r.next(); if(r.getInt(1)!=1)throw new AssertionError("fetch refusal recovery"); }
  }
  System.out.println("PASS");
 }
}
