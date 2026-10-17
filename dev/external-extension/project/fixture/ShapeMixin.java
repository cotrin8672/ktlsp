package fixture;

import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.Shadow;

@Mixin(ShapeTarget.class)
public abstract class ShapeMixin {
    @Shadow public abstract ShapeTarget.Axis shadow$axis(ShapeTarget.Axis value);
    @Shadow public abstract ShapeTarget.Point shadow$point(ShapeTarget.Point value);
}
