package fixture;

// An annotation-free consumer convention, unrelated to Minecraft/Mixin.
// The external test provider links forward$ declarations to the configured target.
public abstract class ForwardLink {
    public abstract int forward$twice(int value);
    public int call(int value) { return forward$twice(value); }
}
