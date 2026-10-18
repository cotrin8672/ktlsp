package outerfixture;
public class Factory {
 public Outer<SpecialItem>.Inner<Other> value;
 public Outer<SpecialItem>.Inner<Other> make() { return null; }
 public Outer<SpecialItem>.Shadow<Other> shadow() { return null; }
 public <T extends Item> T same(T value) { return value; }
}
