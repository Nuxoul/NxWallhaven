package io.github.nuxoul.nxwallhaven.shared

/** A bounded mobile runway; PageAccumulator still owns one-flight/exhaustion/generation guards. */
object GalleryPrefetch {
    fun shouldLoad(total:Int,lastVisible:Int,visibleCount:Int):Boolean =
        total>0 && lastVisible>=0 && (total<72 || total-lastVisible-1<=maxOf(24,visibleCount.coerceIn(1,24)*3))
    fun thumbnailEnd(total:Int,lastVisible:Int,visibleCount:Int):Int =
        if(total<=0 || lastVisible<0)0 else minOf(total,lastVisible+1+maxOf(24,visibleCount.coerceIn(1,24)*3))
}
