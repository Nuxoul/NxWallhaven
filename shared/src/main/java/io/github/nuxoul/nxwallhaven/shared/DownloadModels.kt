package io.github.nuxoul.nxwallhaven.shared

import org.json.JSONArray
import org.json.JSONObject

data class DownloadItem(
    val id:String,val wall:Wall,val state:String,val received:Long=0,val total:Long?=null,
    val uri:String?=null,val created:Long=0,val updated:Long=0,val error:String?=null,
) {
    val active:Boolean get()=state in setOf("queued","running")
    val complete:Boolean get()=state=="completed"
    val percent:Float? get()=total?.takeIf{it>0}?.let{(received.toFloat()/it).coerceIn(0f,1f)}
}
object DownloadWire {
    fun wall(w:Wall):JSONObject=JSONObject().apply {
        put("id",w.id);put("path",w.original);put("resolution",w.resolution)
        put("dimension_x",w.width);put("dimension_y",w.height);put("purity",w.purity)
        put("category",w.category);put("file_size",w.size);put("file_type",w.format)
        put("thumbs",JSONObject().put("original",w.thumbnail));put("colors",JSONArray(w.colors))
        w.uploader?.let{put("uploader",JSONObject().put("username",it))}
        put("tags",JSONArray().apply{w.tags.forEach{(id,name)->put(JSONObject().put("id",id).put("name",name))}})
    }
    fun item(o:JSONObject):DownloadItem=DownloadItem(o.getString("id"),Wire.wall(o.getJSONObject("wall")),o.getString("state"),
        o.optLong("received"),if(o.isNull("total"))null else o.optLong("total"),
        if(o.isNull("uri"))null else o.optString("uri"),o.optLong("created"),o.optLong("updated"),
        if(o.isNull("error"))null else o.optString("error"))
    fun items(json:String):List<DownloadItem>{val a=JSONArray(json);return (0 until a.length()).map{item(a.getJSONObject(it))}}
}
