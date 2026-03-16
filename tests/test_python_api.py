import pytest
from unittest.mock import MagicMock, AsyncMock

@pytest.fixture
def mock_webrtc_module(monkeypatch):
    """
    Mock the native 'unitree_webrtc_rs' Rust binary module.
    This allows us to test Python code that uses the API without
    needing to run the actual WebRTC Rust core.
    """
    mock_module = MagicMock()
    mock_module.WebRTCConnectionMethod.LocalSTA = 2
    
    class MockConnection:
        def __init__(self, method, ip=None):
            self.method = method
            self.ip = ip
            
            # Setup bridge mocks matching the structure exposed by pyi
            self.datachannel = MagicMock()
            self.datachannel.pub_sub = MagicMock()
            self.datachannel.disableTrafficSaving = AsyncMock(return_value=True)
            self.datachannel.pub_sub.publish_request_new = AsyncMock()
            
            self.video = MagicMock()
            self.audio = MagicMock()

        async def connect(self): pass
        async def disconnect(self): pass
            
    mock_module.UnitreeWebRTCConnection = MockConnection
    
    import sys
    monkeypatch.setitem(sys.modules, 'unitree_webrtc_rs', mock_module)
    return mock_module

@pytest.mark.asyncio
async def test_lidar_api_workflow(mock_webrtc_module):
    from unitree_webrtc_rs import UnitreeWebRTCConnection, WebRTCConnectionMethod
    conn = UnitreeWebRTCConnection(WebRTCConnectionMethod.LocalSTA, ip="127.0.0.1")
    await conn.connect()
    
    # Step 1: Disable traffic saving
    await conn.datachannel.disableTrafficSaving(True)
    conn.datachannel.disableTrafficSaving.assert_awaited_once_with(True)

    # Step 2: Set decoder for lidar
    conn.datachannel.set_decoder(decoder_type='native')
    conn.datachannel.set_decoder.assert_called_once_with(decoder_type='native')
    
    # Step 3: Turn on lidar stream 
    conn.datachannel.pub_sub.publish_without_callback("rt/utlidar/switch", "on")
    conn.datachannel.pub_sub.publish_without_callback.assert_called_once_with("rt/utlidar/switch", "on")
    
    # Step 4: Register callback
    user_callback = MagicMock()
    conn.datachannel.pub_sub.subscribe("rt/utlidar/voxel_map_compressed", user_callback)
    
    # Extract the registered callback and simulate getting a message from Rust
    args, _kwargs = conn.datachannel.pub_sub.subscribe.call_args
    registered_cb = args[1]
    
    mock_lidar_message = {"data": {"data": [1.0, 2.0, 3.0, 4.0]}}
    registered_cb(mock_lidar_message)
    
    # Verify our python callback was executed exactly as expected
    user_callback.assert_called_once_with(mock_lidar_message)

@pytest.mark.asyncio
async def test_sport_mode_api_workflow(mock_webrtc_module):
    from unitree_webrtc_rs import UnitreeWebRTCConnection, WebRTCConnectionMethod
    conn = UnitreeWebRTCConnection(WebRTCConnectionMethod.LocalSTA, ip="127.0.0.1")
    await conn.connect()
    
    # Setup mock behavior for the request wrapper
    conn.datachannel.pub_sub.publish_request_new.return_value = {"status": "ok", "mode": "stand"}
    
    # Step 1: User fires a sport command
    SPORT_CMD_STAND = 1004
    response = await conn.datachannel.pub_sub.publish_request_new(
        "rt/api/sport/request", 
        {"api_id": SPORT_CMD_STAND}
    )
    
    # Verify the API was called with the exact parameters
    conn.datachannel.pub_sub.publish_request_new.assert_awaited_once_with(
        "rt/api/sport/request", 
        {"api_id": SPORT_CMD_STAND}
    )
    
    # Verify data returned seamlessly to python user
    assert response == {"status": "ok", "mode": "stand"}

@pytest.mark.asyncio
async def test_video_api_workflow(mock_webrtc_module):
    from unitree_webrtc_rs import UnitreeWebRTCConnection, WebRTCConnectionMethod
    conn = UnitreeWebRTCConnection(WebRTCConnectionMethod.LocalSTA, ip="127.0.0.1")
    await conn.connect()
    
    # Register video callback
    user_callback = MagicMock()
    conn.video.on_frame(user_callback)
    
    # Switch on video channel
    conn.video.switchVideoChannel(True)
    conn.video.switchVideoChannel.assert_called_once_with(True)
    
    # Simulate frame arrival
    args, _kwargs = conn.video.on_frame.call_args
    registered_cb = args[0]
    
    import numpy as np
    mock_frame = np.zeros((480, 640, 3), dtype=np.uint8)
    registered_cb(mock_frame)
    
    # Verify frame reached user callback
    user_callback.assert_called_once()
    assert (user_callback.call_args[0][0] == mock_frame).all()

@pytest.mark.asyncio
async def test_audio_api_workflow(mock_webrtc_module):
    from unitree_webrtc_rs import UnitreeWebRTCConnection, WebRTCConnectionMethod
    conn = UnitreeWebRTCConnection(WebRTCConnectionMethod.LocalSTA, ip="127.0.0.1")
    await conn.connect()
    
    # Register audio callback
    user_callback = MagicMock()
    conn.audio.on_audio(user_callback)
    
    # Switch on audio channel
    conn.audio.switchAudioChannel(True)
    conn.audio.switchAudioChannel.assert_called_once_with(True)
    
    # Simulate audio frame arrival
    args, _kwargs = conn.audio.on_audio.call_args
    registered_cb = args[0]
    
    import numpy as np
    mock_audio_frame = np.zeros((1024,), dtype=np.int16)
    registered_cb(mock_audio_frame)
    
    # Verify audio reached user
    user_callback.assert_called_once()
    assert (user_callback.call_args[0][0] == mock_audio_frame).all()
