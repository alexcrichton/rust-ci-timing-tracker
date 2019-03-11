async function run() {
  const container = document.getElementById('charts');
  const element = document.createElement('div');
  container.appendChild(element);

  const response = await fetch('./overall.json');
  const data = await response.json();

  let max = 0;
  const series = data.series.map((s, i) => {
    s.data = s.data.map((point, i) => {
      if (point > max)
        max = point;
      return [Date.parse(data.commits[i].date), point];
    });
    if (i > 3)
      s.visible = false;
    return s;
  });

  var myChart = Highcharts.chart(element, {
    chart: {
      height: '100%',
    },
    title: {
      text: 'TITLE',
    },
    yAxis: {
      labels: {
        formatter: function() { return format(this.value); },
      },
      title: {
        text: 'Time (seconds)',
      },
    },
    xAxis: {
      type: 'datetime',
      dateTimeLabelFormats: {
        month: '%e. %b',
        year: '%b',
      },
      title: {
        text: 'Commit index',
      },
    },
    tooltip: {
      useHTML: true,
      style: {
        pointerEvents: 'auto',
      },
      headerFormat: '<b>{series.name}</b><br>',
      pointFormatter: function() {
        let text = '' + format(this.y);
        const commit = data.commits[this.index].sha;
        text += '<br>' + commit.substring(0, 8);
        return text;
      },
    },
    series,
    plotOptions: {
      series: {
        point: {
          events: {
            click: event => {
              const commit = data.commits[event.point.index].sha;
              const name = event.point.series.name;
              window.location = `commit.html?job=${name}&commit=${commit}`;
            }
          },
        },
      },
    },
  });

  document.getElementById('loading').remove();
}

document.addEventListener('DOMContentLoaded', run);

function get_mut(map, k, default_) {
  if (map[k] === undefined) {
    map[k] = default_;
  }
  return map[k];
}


function format(s) {
  if (s < 60) {
    return `${Math.floor(s)}s`;
  }
  s /= 60;
  if (s < 60) {
    return `${Math.floor(s * 100) / 100}m`;
  }
  return `${Math.floor(s / 60)}h${Math.floor(s % 60)}m`;
}
