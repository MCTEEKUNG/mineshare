package com.mineshare.mobile

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import org.json.JSONObject

/** Android owns mDNS so LAN permission and Wi-Fi changes follow platform policy. */
class PeerDiscovery(private val context: Context, private val command: (JSONObject) -> Unit) {
    private val nsd = context.getSystemService(NsdManager::class.java)
    private val connectivity = context.getSystemService(ConnectivityManager::class.java)
    private var registration: NsdManager.RegistrationListener? = null
    private var discovery: NsdManager.DiscoveryListener? = null
    private val ids = mutableMapOf<String, String>()
    private val resolutions = mutableMapOf<String, NsdManager.ServiceInfoCallback>()
    private var identity: JSONObject? = null
    private var closed = false
    private val networkCallback = object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(network: Network) { restart() }
        override fun onLost(network: Network) { restart() }
    }

    fun start(event: JSONObject) {
        identity = event
        connectivity.registerDefaultNetworkCallback(networkCallback)
        restart()
    }

    @Synchronized private fun restart() {
        if (closed) return
        stopDiscovery()
        val local = identity ?: return
        val service = NsdServiceInfo().apply {
            serviceName = "MineShare-${local.getString("device_id").take(8)}"
            serviceType = "_mineshare._tcp."
            port = local.getInt("port")
            setAttribute("device_id", local.getString("device_id"))
            setAttribute("display_name", android.os.Build.MODEL)
            setAttribute("os", "android")
            setAttribute("pairing_protocol", "5")
            setAttribute("version", local.optString("version", "0.0.32 · control-v5"))
        }
        val listener = object : NsdManager.RegistrationListener {
            override fun onServiceRegistered(info: NsdServiceInfo) = Unit
            override fun onRegistrationFailed(info: NsdServiceInfo, errorCode: Int) {
                BridgeState.error("Could not announce this phone on Wi-Fi ($errorCode).")
            }
            override fun onServiceUnregistered(info: NsdServiceInfo) = Unit
            override fun onUnregistrationFailed(info: NsdServiceInfo, errorCode: Int) = Unit
        }
        registration = listener
        nsd.registerService(service, NsdManager.PROTOCOL_DNS_SD, listener)
        val browser = object : NsdManager.DiscoveryListener {
            override fun onDiscoveryStarted(type: String) = Unit
            override fun onDiscoveryStopped(type: String) = Unit
            override fun onStartDiscoveryFailed(type: String, code: Int) {
                BridgeState.error("Device discovery needs local network permission ($code).")
            }
            override fun onStopDiscoveryFailed(type: String, code: Int) = Unit
            override fun onServiceFound(info: NsdServiceInfo) {
                if (info.serviceType.trimEnd('.') != "_mineshare._tcp") return
                if (synchronized(resolutions) { resolutions.containsKey(info.serviceName) }) return
                val callback = object : NsdManager.ServiceInfoCallback {
                    override fun onServiceInfoCallbackRegistrationFailed(code: Int) {
                        synchronized(resolutions) { resolutions.remove(info.serviceName) }
                    }
                    override fun onServiceInfoCallbackUnregistered() = Unit
                    override fun onServiceLost() { forget(info.serviceName) }
                    override fun onServiceUpdated(service: NsdServiceInfo) {
                        if (closed) return
                        fun attribute(name: String) = service.attributes[name]?.toString(Charsets.UTF_8).orEmpty()
                        val id = attribute("device_id")
                        if (id.isBlank() || id == local.optString("device_id")) return
                        val host = service.hostAddresses.firstOrNull { it is java.net.Inet4Address }
                            ?: service.hostAddresses.firstOrNull() ?: return
                        synchronized(ids) { ids[service.serviceName] = id }
                        command(JSONObject().put("type", "discover_peer").put("device_id", id)
                            .put("display_name", attribute("display_name").ifBlank { service.serviceName })
                            .put("os", attribute("os")).put("host", host.hostAddress?.substringBefore('%'))
                            .put("scope_id", (host as? java.net.Inet6Address)?.scopeId ?: 0)
                            .put("port", service.port).put("version", attribute("version"))
                            .put("compatible", attribute("pairing_protocol") == "5"))
                    }
                }
                synchronized(resolutions) { resolutions[info.serviceName] = callback }
                nsd.registerServiceInfoCallback(info, context.mainExecutor, callback)
            }
            override fun onServiceLost(info: NsdServiceInfo) {
                synchronized(resolutions) { resolutions.remove(info.serviceName) }?.let {
                    runCatching { nsd.unregisterServiceInfoCallback(it) }
                }
                forget(info.serviceName)
            }
        }
        discovery = browser
        nsd.discoverServices("_mineshare._tcp.", NsdManager.PROTOCOL_DNS_SD, browser)
    }

    private fun stopDiscovery() {
        discovery?.let { runCatching { nsd.stopServiceDiscovery(it) } }
        registration?.let { runCatching { nsd.unregisterService(it) } }
        discovery = null
        registration = null
        val callbacks = synchronized(resolutions) { resolutions.values.toList().also { resolutions.clear() } }
        callbacks.forEach { runCatching { nsd.unregisterServiceInfoCallback(it) } }
    }

    private fun forget(name: String) {
        val id = synchronized(ids) { ids.remove(name) } ?: return
        command(JSONObject().put("type", "peer_lost").put("device_id", id))
    }

    @Synchronized fun close() {
        closed = true
        runCatching { connectivity.unregisterNetworkCallback(networkCallback) }
        stopDiscovery()
    }
}
