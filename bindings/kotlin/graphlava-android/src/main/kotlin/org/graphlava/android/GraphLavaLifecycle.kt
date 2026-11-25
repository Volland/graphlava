package org.graphlava.android

import android.content.Context
import android.content.pm.ApplicationInfo
import android.os.Handler
import android.os.Looper
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.ProcessLifecycleOwner
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import org.graphlava.Database

/**
 * Android lifecycle wiring ([[07-platforms#Android]]): on the process's
 * `ON_STOP` it flushes the pending group commit on the writer dispatcher,
 * after any write transaction running there finishes, so every acknowledged
 * commit, `RELAXED` included, survives a kill without warning. The file lock
 * is kept (Android has no `0xdead10cc`). In debuggable apps it also installs
 * the main-thread guard that makes blocking calls throw.
 */
// @tg: implements:: [[openspec:mobile-bindings#Android background and kill safety]]
object GraphLavaLifecycle {
    private val scope = CoroutineScope(SupervisorJob() + Database.writer)

    /**
     * Call once after opening, from any thread. The lifecycle observer is
     * added on the main thread, as `Lifecycle.addObserver` requires: directly
     * when called there, otherwise posted to the main looper.
     */
    fun install(context: Context, db: Database): DefaultLifecycleObserver {
        if (context.applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE != 0) {
            Database.mainThreadGuard = { Looper.myLooper() == Looper.getMainLooper() }
        }
        val observer = object : DefaultLifecycleObserver {
            override fun onStop(owner: LifecycleOwner) {
                scope.launch { runCatching { db.flush() } }
            }
        }
        val add = { ProcessLifecycleOwner.get().lifecycle.addObserver(observer) }
        if (Looper.myLooper() == Looper.getMainLooper()) add() else Handler(Looper.getMainLooper()).post(add)
        return observer
    }
}
