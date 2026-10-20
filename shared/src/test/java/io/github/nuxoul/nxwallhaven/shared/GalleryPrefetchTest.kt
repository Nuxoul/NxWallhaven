package io.github.nuxoul.nxwallhaven.shared

import org.junit.Assert.*
import org.junit.Test

class GalleryPrefetchTest {
    @Test fun preparesThreePagesBeforeScrollingAndKeepsSeveralScreensAhead() {
        assertTrue(GalleryPrefetch.shouldLoad(24,5,6))
        assertTrue(GalleryPrefetch.shouldLoad(48,5,6))
        assertFalse(GalleryPrefetch.shouldLoad(72,5,6))
        assertTrue(GalleryPrefetch.shouldLoad(72,49,6))
        assertTrue(GalleryPrefetch.shouldLoad(96,61,12))
        assertFalse(GalleryPrefetch.shouldLoad(0,-1,0))
        assertFalse(GalleryPrefetch.shouldLoad(72,-1,0))
    }
    @Test fun thumbnailRunwayIsBoundedAndDoesNotDemandOriginals() {
        assertEquals(30,GalleryPrefetch.thumbnailEnd(72,5,6))
        assertEquals(72,GalleryPrefetch.thumbnailEnd(72,55,10))
        assertEquals(0,GalleryPrefetch.thumbnailEnd(0,-1,0))
    }
}
