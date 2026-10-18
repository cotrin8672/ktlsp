package outerfixture;
public class Outer<T extends Item> {
 public class Inner<U extends Other> {
  public T outer() { return null; }
  public U inner() { return null; }
  public <T extends Other> T shadow(T value) { return value; }
 }
 public class Shadow<T extends Other> {
  public T own() { return null; }
 }
}
