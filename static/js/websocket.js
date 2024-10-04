
let socket = new WebSocket("http://127.0.0.1:8000/ws");

socket.onmessage = function(msg) {
	console.log(msg);
}
