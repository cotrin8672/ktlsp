package fixture

import org.spongepowered.asm.mixin.Mixin
import org.spongepowered.asm.mixin.Shadow

@Mixin(ShapeTarget::class)
abstract class ShapeKotlinMixin {
    @Shadow abstract fun axis(value: ShapeTarget.Axis): ShapeTarget.Axis
    @Shadow abstract fun point(value: ShapeTarget.Point): ShapeTarget.Point
    @Shadow abstract fun any(value: Any): Any
    @Shadow abstract fun text(value: String): String
}
