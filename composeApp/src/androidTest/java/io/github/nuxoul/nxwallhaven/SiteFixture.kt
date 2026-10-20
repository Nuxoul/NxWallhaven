package io.github.nuxoul.nxwallhaven

import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket
import java.net.URLDecoder
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import org.json.JSONArray
import org.json.JSONObject

/** A local account, never the person's real Wallhaven account. No secrets are logged. */
class SiteFixture:AutoCloseable {
    data class Row(val id:Int,var label:String,var public:Boolean)
    private val server=ServerSocket(0,16,InetAddress.getByName("127.0.0.1"))
    private val executor=Executors.newCachedThreadPool()
    private val running=AtomicBoolean(true)
    val origin="http://127.0.0.1:${server.localPort}"
    val paths=CopyOnWriteArrayList<String>()
    val posts=AtomicInteger()
    val rows=CopyOnWriteArrayList(listOf(Row(12,"Default",false),Row(13,"Public",true)))
    val favorite=AtomicBoolean(false)
    val dropNextWrite=AtomicBoolean(false)
    val username="FixtureUser"
    init{executor.execute{while(running.get()){try{val socket=server.accept();executor.execute{try{serve(socket)}catch(_:java.io.IOException){/* Client may cancel an oversized response; no fixture mutation is retried. */}}}catch(_:java.io.IOException){if(running.get())throw IllegalStateException("fixture server failed")}}}}
    private fun panel()="""<div id="userpanel"><a href="/user/$username">$username</a><form action="/logout"></form></div>"""
    private fun listing():String=panel()+"""<meta name="csrf-token" content="fixture-token"><div id="collections">"""+rows.joinToString(""){row->
        """<a class="collection ${if(row.public)"public"else"private"}" data-public="${if(row.public)1 else 0}" href="/favorites/${row.id}"><span class="collection-label">${row.label}</span><small>2</small></a>"""
    }+"""</div><form action="/collections/create" method="post"><input type="hidden" name="_token" value="fixture-token"><input name="label"><input name="public"></form>"""
    private fun content(id:Int):String=listing()+"""<div id="thumbs"><figure class="thumb-sfw thumb-general" data-wallpaper-id="abc123"><img src="https://th.wallhaven.cc/small/ab/abc123.jpg"><span class="wall-res">1920 x 1080</span></figure><figure class="thumb-nsfw" data-wallpaper-id="nsf123"><span class="wall-res">1200 x 2000</span></figure></div><ul data-pagination='{"total":1,"current":1}'></ul><form action="/collections/$id" method="post"><input type="hidden" name="_token" value="fixture-token"><input type="hidden" name="_method" value="PATCH"><input name="label"><input name="public"></form><form action="/collections/$id/delete" method="post"><input type="hidden" name="_token" value="fixture-token"><input type="hidden" name="_method" value="DELETE"></form>"""
    private fun serve(socket:Socket){socket.use{s->
        s.soTimeout=10_000
        val input=s.getInputStream();val header=ArrayList<Byte>()
        while(header.size<65536){val n=input.read();if(n<0)return;header.add(n.toByte());if(header.takeLast(4)==listOf(13.toByte(),10.toByte(),13.toByte(),10.toByte()))break}
        val text=String(header.toByteArray(),Charsets.ISO_8859_1)
        val lines=text.split("\r\n");val first=lines.first().split(' ');val method=first[0];val target=first[1];val path=target.substringBefore('?')
        paths+=target
        val headers=lines.drop(1).mapNotNull{it.split(":",limit=2).takeIf{it.size==2}?.let{p->p[0].lowercase() to p[1].trim()}}.toMap()
        val size=headers["content-length"]?.toIntOrNull()?:0;require(size<=65536)
        val body=input.readNBytes(size).toString(Charsets.UTF_8)
        val fields=body.split('&').filter{it.isNotBlank()}.associate{val p=it.split('=',limit=2);URLDecoder.decode(p[0],"UTF-8") to URLDecoder.decode(p.getOrElse(1){""},"UTF-8")}
        var status="200 OK";var extra="";var reply=""
        val logged=headers["cookie"].orEmpty().contains("nxfixture=logged")
        when {
            path=="/login" -> reply="""<html><title>Local fixture login</title><body><form method="post" action="/fixture-login"><button id="signin" type="submit">登录测试账号</button></form></body></html>"""
            path=="/fixture-login" && method=="POST" -> {extra="Set-Cookie: nxfixture=logged; Max-Age=3600; Path=/; HttpOnly; SameSite=Strict\r\n";reply=panel()}
            path=="/too-large" -> reply="x".repeat(2*1024*1024+1)
            !logged -> reply="""<div id="userpanel"><a href="/login">Login</a></div>"""
            method=="POST" -> {
                check(headers["x-csrf-token"]=="fixture-token")
                posts.incrementAndGet()
                when {
                    path=="/favorite/add/abc123" -> favorite.set(true)
                    path=="/favorite/remove/abc123" -> favorite.set(false)
                    path=="/collections/create" -> rows.add(Row(14,fields["label"].orEmpty(),fields["public"]=="1"))
                    path.startsWith("/collections/") -> {
                        val id=path.split('/')[2].toInt()
                        if(fields["_method"]=="DELETE")rows.removeIf{it.id==id}
                        else rows.find{it.id==id}?.let{it.label=fields["label"].orEmpty();it.public=fields["public"]=="1"}
                    }
                    else -> error("unexpected fixture POST")
                }
                if(dropNextWrite.getAndSet(false))return
                reply="""{"status":true}"""
            }
            path=="/favorites" -> reply=listing()
            path.startsWith("/favorites/") -> reply=content(path.substringAfterLast('/').toInt())
            path=="/w/abc123" -> reply=panel()+"""<meta name="csrf-token" content="fixture-token">"""+if(favorite.get())"""<a id="fav-button" href="/favorite/remove/abc123">Remove</a>"""else"""<div id="fav-button" class="add-button"><a class="add-fav" href="/favorite/add/abc123">Default</a></div>"""
            path=="/" -> reply=panel()
            else -> {status="404 Not Found";reply="missing"}
        }
        val bytes=reply.toByteArray(Charsets.UTF_8)
        val out=s.getOutputStream()
        out.write("HTTP/1.1 $status\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: ${bytes.size}\r\nConnection: close\r\n$extra\r\n".toByteArray())
        out.write(bytes);out.flush()
    }}
    override fun close(){running.set(false);server.close();executor.shutdownNow()}
}
