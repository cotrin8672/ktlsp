package defaultfixture;

class Base {
    protected float getBreakSpeed() { return 1; }
}

abstract class Mid extends Base implements Overlay {}

public abstract class JavaDefaults extends Mid {
    float use() { return super.getBreakSpeed(); }
    Object generic(Overlay value) { return value.tooltip("ok"); }
}
