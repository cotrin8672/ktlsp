package fixture;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.Shadow;
@Mixin(Target.class)
public abstract class CaseMixin {
    @Shadow(prefix = "shadow$")
    public abstract int shadow$twice(int value);
    public int call(int value) { return shadow$twice(value); }
}
