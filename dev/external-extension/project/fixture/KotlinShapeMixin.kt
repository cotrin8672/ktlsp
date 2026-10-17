package fixture

import org.spongepowered.asm.mixin.Mixin
import org.spongepowered.asm.mixin.Shadow

@Mixin(KotlinShapeTarget::class)
abstract class KotlinShapeMixin {
    @Shadow abstract fun axis(value: KotlinShapeTarget.Axis): KotlinShapeTarget.Axis
    @Shadow abstract fun marker(value: KotlinShapeTarget.Marker): KotlinShapeTarget.Marker
}
