#!/usr/bin/env python3
"""
Video streaming example for unitree-webrtc-rs
Mirrors the API style of unitree_webrtc_connect but uses Rust backend
"""
import asyncio
import argparse
import numpy as np
import time
from queue import Queue
import threading

try:
    import cv2
    CV2_AVAILABLE = True
except ImportError:
    CV2_AVAILABLE = False
    print("WARNING: opencv-python not installed. Video display will be disabled.")
    print("Install with: pip install opencv-python")

from unitree_webrtc_rs import UnitreeWebRTCConnection, WebRTCConnectionMethod


def main():
    parser = argparse.ArgumentParser(description="Stream video from Unitree Go2 robot")
    parser.add_argument("--ip", type=str, required=True, help="Robot IP address")
    args = parser.parse_args()

    frame_queue = Queue()
    frame_count = 0
    fps_start_time = None
    total_frames = 0
    
    def on_video_frame(frame: np.ndarray):
        nonlocal total_frames
        total_frames += 1
        frame_queue.put((frame, time.time()))
    
    async def async_setup():
        conn = UnitreeWebRTCConnection(
            WebRTCConnectionMethod.LocalSTA,
            ip=args.ip
        )
        
        print(f"Connecting to robot at {args.ip}...")
        await conn.connect()
        print("Connected!")
        
        conn.video.on_frame(on_video_frame)
        
        print("Enabling video channel...")
        conn.video.switchVideoChannel(True)
        print("Video streaming started. Press 'q' to quit.\n")
        
        # Keep connection alive
        try:
            while True:
                await asyncio.sleep(1)
        except:
            pass
        finally:
            conn.video.switchVideoChannel(False)
            await conn.disconnect()
    
    def run_async_thread():
        loop = asyncio.new_event_loop()
        asyncio.set_event_loop(loop)
        loop.run_until_complete(async_setup())
    
    # Start async thread
    async_thread = threading.Thread(target=run_async_thread, daemon=True)
    async_thread.start()
    
    # Main thread: display frames
    try:
        test_start = time.time()
        fps_window_start = time.time()
        fps_window_frames = 0
        
        while True:
            if not frame_queue.empty():
                frame, recv_time = frame_queue.get()
                frame_count += 1
                fps_window_frames += 1
                
                # Calculate instantaneous FPS every second
                elapsed = time.time() - fps_window_start
                if elapsed >= 1.0:
                    fps = fps_window_frames / elapsed
                    print(f"Frame {frame_count}: FPS={fps:.2f} | Shape={frame.shape} dtype={frame.dtype}")
                    fps_window_start = time.time()
                    fps_window_frames = 0
                
                if CV2_AVAILABLE:
                    cv2.imshow('Go2 Video Stream (Rust)', frame)
                    if cv2.waitKey(1) & 0xFF == ord('q'):
                        print("\nUser requested quit")
                        break
            else:
                time.sleep(0.01)
            
            # Auto-stop after 20 seconds for testing
            if time.time() - test_start > 20.0:
                print(f"\n20 second test completed.")
                break
                
    except KeyboardInterrupt:
        print("\nStopping...")
    finally:
        total_time = time.time() - test_start
        avg_fps = frame_count / total_time if total_time > 0 else 0
        print(f"Total frames: {frame_count}, Time: {total_time:.2f}s, Avg FPS: {avg_fps:.2f}")
        if CV2_AVAILABLE:
            cv2.destroyAllWindows()
        print("Disconnected")


if __name__ == "__main__":
    main()
